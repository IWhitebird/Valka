use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Sse, sse::Event},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::info;

use crate::api::Api;
use crate::cluster::{self, NodeInfo, StorageCache};
use crate::convert::log_line_to_json;
use valka_core::{ServerError, TaskStatus};
use valka_engine::state::SignalStatus;
use valka_engine::{CreateTask, Engine, LogIngester};

// ─── Structured Error Response ──────────────────────────────────────

#[derive(Serialize)]
struct ErrorBody {
    error: String,
    code: String,
}

pub enum ApiError {
    NotFound(String),
    InvalidState(String),
    BadRequest(String),
    Conflict(String),
    Unavailable(String),
    TooManyRequests(String),
    Internal(String),
}

impl From<ServerError> for ApiError {
    fn from(e: ServerError) -> Self {
        match e {
            ServerError::TaskNotFound(_)
            | ServerError::WorkerNotFound(_)
            | ServerError::QueueNotFound(_) => ApiError::NotFound(e.to_string()),
            ServerError::InvalidStatusTransition { .. }
            | ServerError::TaskCancelled(_)
            | ServerError::LeaseExpired(_) => ApiError::InvalidState(e.to_string()),
            ServerError::IdempotencyConflict(_) => ApiError::Conflict(e.to_string()),
            ServerError::InvalidArgument(_) => ApiError::BadRequest(e.to_string()),
            ServerError::ResourceExhausted(_) => ApiError::TooManyRequests(e.to_string()),
            ServerError::NotOwner(_) | ServerError::Unavailable(_) => {
                ApiError::Unavailable(e.to_string())
            }
            ServerError::Storage(_) | ServerError::Internal(_) => ApiError::Internal(e.to_string()),
        }
    }
}

impl From<valka_wal::WalError> for ApiError {
    fn from(e: valka_wal::WalError) -> Self {
        ServerError::from(e).into()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, code, message) = match self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, "NOT_FOUND", msg),
            ApiError::InvalidState(msg) => (StatusCode::UNPROCESSABLE_ENTITY, "INVALID_STATE", msg),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "BAD_REQUEST", msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, "CONFLICT", msg),
            ApiError::Unavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE", msg),
            ApiError::TooManyRequests(msg) => {
                (StatusCode::TOO_MANY_REQUESTS, "RESOURCE_EXHAUSTED", msg)
            }
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", msg),
        };
        (
            status,
            Json(ErrorBody {
                error: message,
                code: code.to_string(),
            }),
        )
            .into_response()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) api: Api,
    pub(crate) engine: Engine,
    pub(crate) event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    pub(crate) logs: Arc<LogIngester>,
    pub(crate) metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    pub(crate) node_info: NodeInfo,
    pub(crate) storage_cache: Arc<StorageCache>,
    pub(crate) admin_token: Option<String>,
}

/// Header set when a cluster-wide answer is missing some node's part (gRPC: metadata).
pub(crate) const PARTIAL_HEADER: &str = "x-valka-partial";

/// The partial header's value: `false`, or `true; missing=<nodes>`.
pub(crate) fn partial_value(missing: &[String]) -> String {
    if missing.is_empty() {
        "false".to_string()
    } else {
        format!("true; missing={}", missing.join(","))
    }
}

pub(crate) fn partial_headers(missing: &[String]) -> [(axum::http::HeaderName, String); 1] {
    [(
        axum::http::HeaderName::from_static(PARTIAL_HEADER),
        partial_value(missing),
    )]
}

/// Build the API router (useful for testing with tower::ServiceExt::oneshot)
pub fn build_api_router(
    api: Api,
    event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    logs: Arc<LogIngester>,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    node_info: NodeInfo,
    admin_token: Option<String>,
    api_keys: crate::auth::ApiKeys,
) -> Router {
    let state = AppState {
        engine: api.engine().clone(),
        api,
        event_tx,
        logs,
        metrics_handle,
        node_info,
        storage_cache: cluster::new_storage_cache(),
        admin_token,
    };
    let admin_routes = Router::new()
        .route(
            "/api/v1/cluster/nodes/{node}/drain",
            post(crate::admin::drain_node),
        )
        .route(
            "/api/v1/cluster/shards/{shard}/move",
            post(crate::admin::move_shard),
        )
        .route(
            "/api/v1/cluster/shards/{shard}/pin",
            delete(crate::admin::unpin_shard),
        )
        .route(
            "/api/v1/queues/{queue}/partitions",
            put(crate::admin::set_partitions),
        )
        .route("/api/v1/cluster/audit", get(crate::admin::audit_log))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::admin::require_admin,
        ));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Public API routes: behind the API keys when they are configured.
    let public_routes = Router::new()
        .route(
            "/api/v1/tasks",
            post(create_task).get(list_tasks).delete(clear_all_tasks),
        )
        .route("/api/v1/tasks/{task_id}", get(get_task).delete(delete_task))
        .route("/api/v1/tasks/{task_id}/cancel", post(cancel_task))
        .route("/api/v1/tasks/{task_id}/signal", post(send_signal))
        .route("/api/v1/tasks/{task_id}/signals", get(list_signals))
        .route("/api/v1/tasks/{task_id}/runs", get(get_task_runs))
        .route(
            "/api/v1/tasks/{task_id}/checkpoints",
            get(get_task_checkpoints),
        )
        .route(
            "/api/v1/tasks/{task_id}/runs/{run_id}/logs",
            get(get_run_logs),
        )
        .route("/api/v1/workers", get(list_workers))
        .route("/api/v1/dead-letters", get(list_dead_letters))
        .route("/api/v1/events", get(subscribe_events_sse))
        .route("/api/v1/cluster", get(cluster::cluster_overview))
        .route("/api/v1/cluster/shards", get(cluster::list_shards))
        .route("/api/v1/cluster/shards/{shard}", get(cluster::get_shard))
        .route("/api/v1/cluster/storage", get(cluster::storage))
        .route("/api/v1/cluster/snapshot", post(cluster::snapshot_now))
        .route("/api/v1/cluster/members", get(cluster::members))
        .route("/api/v1/cluster/placement", get(crate::admin::placement))
        .route("/api/v1/queues", get(crate::admin::list_queues))
        .route_layer(axum::middleware::from_fn_with_state(
            api_keys,
            crate::auth::require_api_key,
        ));

    Router::new()
        .merge(public_routes)
        .merge(admin_routes)
        .route("/metrics", get(metrics))
        .route("/healthz", get(healthz))
        .with_state(state)
        .layer(cors)
}

#[allow(clippy::too_many_arguments)]
pub async fn serve_rest(
    addr: SocketAddr,
    api: Api,
    event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    logs: Arc<LogIngester>,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    node_info: NodeInfo,
    admin_token: Option<String>,
    api_keys: crate::auth::ApiKeys,
    web_dir: String,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    let api_routes = build_api_router(
        api,
        event_tx,
        logs,
        metrics_handle,
        node_info,
        admin_token,
        api_keys,
    );

    let index_path = format!("{web_dir}/index.html");
    let spa_fallback = ServeDir::new(&web_dir).not_found_service(ServeFile::new(index_path));
    let app = api_routes.fallback_service(spa_fallback);

    info!("REST server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await?;

    Ok(())
}

#[derive(Deserialize)]
struct CreateTaskBody {
    queue_name: String,
    task_name: String,
    #[serde(default)]
    input: Option<serde_json::Value>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_max_retries")]
    max_retries: i32,
    #[serde(default = "default_timeout")]
    timeout_seconds: i32,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    scheduled_at: Option<String>,
}

fn default_max_retries() -> i32 {
    3
}
fn default_timeout() -> i32 {
    300
}

#[derive(Deserialize)]
struct ListTasksQuery {
    #[serde(default)]
    queue_name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    50
}

async fn create_task(
    State(state): State<AppState>,
    Json(body): Json<CreateTaskBody>,
) -> Result<impl IntoResponse, ApiError> {
    let scheduled_at = match body.scheduled_at.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            s.parse::<chrono::DateTime<chrono::Utc>>()
                .map_err(|e| ApiError::BadRequest(format!("Invalid scheduled_at: {e}")))?,
        ),
    };
    let (task, replayed) = state
        .api
        .create_task_replayed(CreateTask {
            queue_name: body.queue_name,
            task_name: body.task_name,
            input: body.input,
            priority: body.priority,
            max_retries: body.max_retries,
            timeout_seconds: body.timeout_seconds,
            idempotency_key: body.idempotency_key.filter(|k| !k.is_empty()),
            metadata: body.metadata.unwrap_or(serde_json::json!({})),
            scheduled_at,
        })
        .await?;
    // A create with an idempotency key already in use answers with that key's task.
    let status = if replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        [(
            axum::http::HeaderName::from_static("idempotent-replayed"),
            if replayed { "true" } else { "false" },
        )],
        Json(task.to_json()),
    ))
}

async fn get_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let task = state
        .api
        .get_task(&task_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Task not found".to_string()))?;
    Ok(Json(task.to_json()))
}

async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<ListTasksQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let status = match query.status.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            TaskStatus::from_str_status(s)
                .ok_or_else(|| ApiError::BadRequest(format!("Unknown status {s}")))?,
        ),
    };
    let tasks = state
        .api
        .list_tasks(
            query.queue_name.as_deref(),
            status,
            query.limit.clamp(1, 1000) as usize,
            query.offset.clamp(0, 100_000) as usize,
        )
        .await?;
    let result: Vec<serde_json::Value> = tasks.value.iter().map(|t| t.to_json()).collect();
    Ok((partial_headers(&tasks.missing), Json(result)))
}

async fn cancel_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let task = state
        .api
        .cancel_task(&task_id, "Cancelled by user")
        .await
        .map_err(|e| match e {
            ServerError::TaskNotFound(_) | ServerError::InvalidStatusTransition { .. } => {
                ApiError::InvalidState("Task not found or not in cancellable state".to_string())
            }
            other => other.into(),
        })?;
    Ok(Json(task.to_json()))
}

#[derive(Deserialize)]
struct SendSignalBody {
    signal_name: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

async fn send_signal(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(body): Json<SendSignalBody>,
) -> Result<impl IntoResponse, ApiError> {
    let (signal, delivered) = state
        .api
        .send_signal(&task_id, &body.signal_name, body.payload)
        .await
        .map_err(|e| match e {
            ServerError::InvalidStatusTransition { from, .. } => {
                ApiError::InvalidState(format!("Cannot send signal to task in {from} state"))
            }
            other => other.into(),
        })?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "signal_id": signal.id,
            "delivered": delivered,
        })),
    ))
}

#[derive(Deserialize)]
struct ListSignalsQuery {
    #[serde(default)]
    status: Option<String>,
}

async fn list_signals(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Query(query): Query<ListSignalsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let status = match query.status.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            SignalStatus::parse(s)
                .ok_or_else(|| ApiError::BadRequest(format!("Unknown status {s}")))?,
        ),
    };
    let result: Vec<serde_json::Value> = state
        .api
        .list_signals(&task_id, status)
        .await?
        .iter()
        .map(|s| s.to_json())
        .collect();
    Ok(Json(result))
}

async fn delete_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let deleted = state.api.delete_task(&task_id).await?;
    if !deleted {
        return Err(ApiError::NotFound("Task not found".to_string()));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn clear_all_tasks(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let count = state.api.clear_all_tasks().await?;
    Ok((
        partial_headers(&count.missing),
        Json(serde_json::json!({ "deleted_count": count.value })),
    ))
}

async fn get_task_runs(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let runs = state.api.runs(&task_id).await?.unwrap_or_default();
    let result: Vec<serde_json::Value> = runs.iter().map(|r| r.to_json()).collect();
    Ok(Json(result))
}

async fn get_task_checkpoints(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let checkpoints = state
        .api
        .checkpoints(&task_id)
        .await?
        .ok_or(ServerError::TaskNotFound(task_id))?;
    let result: Vec<serde_json::Value> = checkpoints.iter().map(|c| c.to_json()).collect();
    Ok(Json(result))
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default = "default_log_limit")]
    limit: i64,
    #[serde(default)]
    after_id: Option<i64>,
}

fn default_log_limit() -> i64 {
    1000
}

async fn get_run_logs(
    State(state): State<AppState>,
    Path((_task_id, run_id)): Path<(String, String)>,
    Query(query): Query<LogsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let after = query.after_id.map(|a| a.max(0) as usize).unwrap_or(0);
    let limit = query.limit.clamp(1, 10_000) as usize;
    let lines = state.logs.read(&run_id, after + limit).await;
    // Log ids are 1-based positions within the run's log, so `after_id` paging works.
    let result: Vec<serde_json::Value> = lines
        .iter()
        .enumerate()
        .skip(after)
        .take(limit)
        .map(|(i, l)| log_line_to_json(l, i + 1))
        .collect();
    Ok(Json(result))
}

async fn list_workers(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let workers = state.api.workers().await?;
    let rows: Vec<serde_json::Value> = workers
        .value
        .iter()
        .map(|w| {
            serde_json::json!({
                "id": w.id,
                "node_id": w.node_id,
                "name": w.name,
                "queues": w.queues,
                "concurrency": w.concurrency,
                "active_tasks": w.active_tasks,
                "status": "CONNECTED",
                "last_heartbeat": w.last_heartbeat.to_rfc3339(),
                "connected_at": w.connected_at.to_rfc3339(),
            })
        })
        .collect();
    Ok((partial_headers(&workers.missing), Json(rows)))
}

#[derive(Deserialize)]
struct DeadLetterQuery {
    #[serde(default)]
    queue_name: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

async fn list_dead_letters(
    State(state): State<AppState>,
    Query(query): Query<DeadLetterQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let dls = state
        .api
        .dead_letters(
            query.queue_name.as_deref(),
            query.limit.clamp(1, 1000) as usize,
            query.offset.clamp(0, 100_000) as usize,
        )
        .await?;
    let result: Vec<serde_json::Value> = dls.value.iter().map(|d| d.to_json()).collect();
    Ok((partial_headers(&dls.missing), Json(result)))
}

async fn subscribe_events_sse(
    State(state): State<AppState>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.event_tx.subscribe();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let data = serde_json::json!({
                        "event_id": event.event_id,
                        "task_id": event.task_id,
                        "queue_name": event.queue_name,
                        "new_status": event.new_status,
                        "timestamp_ms": event.timestamp_ms,
                    });
                    yield Ok(Event::default().data(data.to_string()));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream)
}

async fn metrics(State(state): State<AppState>) -> String {
    state.metrics_handle.render()
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    match state.engine.poisoned() {
        None => (StatusCode::OK, "ok"),
        Some(_) => (StatusCode::SERVICE_UNAVAILABLE, "wal writer poisoned"),
    }
}
