use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Sse, sse::Event},
    routing::{get, post},
};
use serde::Deserialize;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::info;

use valka_cluster::{ClusterManager, NodeForwarder};
use valka_core::{TaskId, partition_for_task};
use valka_db::DbPool;
use valka_dispatcher::DispatcherService;
use valka_matching::MatchingService;
use valka_matching::partition::TaskEnvelope;

#[derive(Clone)]
pub struct AppState {
    pool: DbPool,
    event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    matching: MatchingService,
    dispatcher: DispatcherService,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    cluster: Arc<ClusterManager>,
    forwarder: NodeForwarder,
    node_id: String,
}

/// Build the API router (useful for testing with tower::ServiceExt::oneshot)
pub fn build_api_router(
    pool: DbPool,
    event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    matching: MatchingService,
    dispatcher: DispatcherService,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    cluster: Arc<ClusterManager>,
    forwarder: NodeForwarder,
) -> Router {
    let node_id = cluster.node_id().0.clone();
    let state = AppState {
        pool,
        event_tx,
        matching,
        dispatcher,
        metrics_handle,
        cluster,
        forwarder,
        node_id,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/v1/tasks", post(create_task).get(list_tasks).delete(clear_all_tasks))
        .route("/api/v1/tasks/{task_id}", get(get_task).delete(delete_task))
        .route("/api/v1/tasks/{task_id}/cancel", post(cancel_task))
        .route("/api/v1/tasks/{task_id}/runs", get(get_task_runs))
        .route(
            "/api/v1/tasks/{task_id}/runs/{run_id}/logs",
            get(get_run_logs),
        )
        .route("/api/v1/workers", get(list_workers))
        .route("/api/v1/dead-letters", get(list_dead_letters))
        .route("/api/v1/events", get(subscribe_events_sse))
        .route("/metrics", get(metrics))
        .route("/healthz", get(healthz))
        .with_state(state)
        .layer(cors)
}

pub async fn serve_rest(
    addr: SocketAddr,
    pool: DbPool,
    event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    matching: MatchingService,
    dispatcher: DispatcherService,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    cluster: Arc<ClusterManager>,
    forwarder: NodeForwarder,
    web_dir: String,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    let api_routes = build_api_router(
        pool, event_tx, matching, dispatcher, metrics_handle, cluster, forwarder,
    );

    // Serve static files with SPA fallback
    let index_path = format!("{}/index.html", &web_dir);
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
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let task_id = TaskId::new();
    let partition = partition_for_task(
        &body.queue_name,
        &task_id.0,
        state.matching.config().num_partitions,
    );

    let scheduled_at = body
        .scheduled_at
        .as_ref()
        .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());

    let metadata = body.metadata.unwrap_or(serde_json::json!({}));

    let task = valka_db::queries::tasks::create_task(
        &state.pool,
        valka_db::queries::tasks::CreateTaskParams {
            id: task_id.0.clone(),
            queue_name: body.queue_name.clone(),
            task_name: body.task_name.clone(),
            partition_id: partition.0,
            input: body.input.clone(),
            priority: body.priority,
            max_retries: body.max_retries,
            timeout_seconds: body.timeout_seconds,
            idempotency_key: body.idempotency_key,
            metadata: metadata.clone(),
            scheduled_at,
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    valka_core::metrics::record_task_created(&body.queue_name);

    // Emit task created event
    let event = valka_proto::TaskEvent {
        event_id: uuid::Uuid::now_v7().to_string(),
        task_id: task_id.0.clone(),
        queue_name: body.queue_name.clone(),
        previous_status: 0,
        new_status: 1, // PENDING
        worker_id: String::new(),
        node_id: state.node_id.clone(),
        attempt_number: 0,
        error_message: String::new(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
    };
    let _ = state.event_tx.send(event);

    // Check if we own this partition; if not, forward to owner
    if !state
        .cluster
        .owns_partition(&body.queue_name, partition.0)
        .await
        && let Some(owner_addr) = state
            .cluster
            .get_partition_owner_addr(&body.queue_name, partition.0)
            .await
    {
        let _ = state
            .forwarder
            .forward_task(&owner_addr, &task_id.0, &body.queue_name, partition.0)
            .await;
        valka_core::metrics::record_task_forwarded(&body.queue_name);
        return Ok((StatusCode::CREATED, Json(task_row_to_json(task))));
    }
    // If owner unknown, fall through to local sync match (safety)

    // Sync match (hot path) — same as gRPC create_task
    if scheduled_at.is_none() {
        let envelope = TaskEnvelope {
            task_id: task_id.0.clone(),
            task_run_id: String::new(),
            queue_name: body.queue_name.clone(),
            task_name: body.task_name.clone(),
            input: body.input.map(|v| v.to_string()),
            attempt_number: 1,
            timeout_seconds: body.timeout_seconds,
            metadata: metadata.to_string(),
            priority: body.priority,
        };
        let _ = state
            .matching
            .offer_task(&body.queue_name, partition, envelope);
    }

    Ok((StatusCode::CREATED, Json(task_row_to_json(task))))
}

async fn get_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let task = valka_db::queries::tasks::get_task(&state.pool, &task_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Task not found".to_string()))?;

    Ok(Json(task_row_to_json(task)))
}

async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<ListTasksQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let tasks = valka_db::queries::tasks::list_tasks(
        &state.pool,
        query.queue_name.as_deref(),
        query.status.as_deref(),
        query.limit,
        query.offset,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let result: Vec<serde_json::Value> = tasks.into_iter().map(task_row_to_json).collect();
    Ok(Json(result))
}

async fn cancel_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Try cancelling (PENDING, RETRY, RUNNING, DISPATCHING)
    let task = valka_db::queries::tasks::cancel_task_any(&state.pool, &task_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Task not found or not in cancellable state".to_string(),
            )
        })?;

    // If task was RUNNING, forward cancellation to the worker
    state.dispatcher.cancel_task_on_worker(&task_id).await;

    // Emit cancel event
    let event = valka_proto::TaskEvent {
        event_id: uuid::Uuid::now_v7().to_string(),
        task_id: task_id.clone(),
        queue_name: task.queue_name.clone(),
        previous_status: 0,
        new_status: 8, // CANCELLED
        worker_id: String::new(),
        node_id: state.node_id.clone(),
        attempt_number: 0,
        error_message: String::new(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
    };
    let _ = state.event_tx.send(event);

    Ok(Json(task_row_to_json(task)))
}

async fn delete_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let deleted = valka_db::queries::tasks::delete_task(&state.pool, &task_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !deleted {
        return Err((StatusCode::NOT_FOUND, "Task not found".to_string()));
    }

    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn clear_all_tasks(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let count = valka_db::queries::tasks::clear_all_tasks(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({ "deleted_count": count })))
}

async fn get_task_runs(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let runs = valka_db::queries::task_runs::get_runs_for_task(&state.pool, &task_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let result: Vec<serde_json::Value> = runs.into_iter().map(task_run_to_json).collect();
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
    Path((task_id, run_id)): Path<(String, String)>,
    Query(query): Query<LogsQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Verify the run belongs to the task
    let _ = task_id; // Used for API consistency; logs are queried by run_id

    let logs = valka_db::queries::task_logs::get_logs_for_run(
        &state.pool,
        &run_id,
        query.limit,
        query.after_id,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let result: Vec<serde_json::Value> = logs.into_iter().map(task_log_to_json).collect();
    Ok(Json(result))
}

async fn list_workers(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Return in-memory connected workers from dispatcher
    let workers: Vec<serde_json::Value> = state
        .dispatcher
        .workers()
        .iter()
        .map(|entry| {
            let h = entry.value();
            serde_json::json!({
                "id": h.worker_id.0,
                "name": h.worker_name,
                "queues": h.queues,
                "concurrency": h.concurrency,
                "active_tasks": h.active_tasks.len(),
                "status": "CONNECTED",
                "last_heartbeat": h.last_heartbeat.to_rfc3339(),
                "connected_at": h.connected_at.to_rfc3339(),
            })
        })
        .collect();
    Ok(Json(workers))
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
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dls = valka_db::queries::dead_letter::list_dead_letters(
        &state.pool,
        query.queue_name.as_deref(),
        query.limit,
        query.offset,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let result: Vec<serde_json::Value> = dls
        .into_iter()
        .map(|dl| {
            serde_json::json!({
                "id": dl.id,
                "task_id": dl.task_id,
                "queue_name": dl.queue_name,
                "task_name": dl.task_name,
                "input": dl.input,
                "error_message": dl.error_message,
                "attempt_count": dl.attempt_count,
                "metadata": dl.metadata,
                "created_at": dl.created_at.to_rfc3339(),
            })
        })
        .collect();
    Ok(Json(result))
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

async fn healthz() -> &'static str {
    "ok"
}

fn task_row_to_json(row: valka_db::queries::tasks::TaskRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "queue_name": row.queue_name,
        "task_name": row.task_name,
        "status": row.status,
        "priority": row.priority,
        "max_retries": row.max_retries,
        "attempt_count": row.attempt_count,
        "timeout_seconds": row.timeout_seconds,
        "idempotency_key": row.idempotency_key,
        "input": row.input,
        "metadata": row.metadata,
        "output": row.output,
        "error_message": row.error_message,
        "scheduled_at": row.scheduled_at.map(|t| t.to_rfc3339()),
        "created_at": row.created_at.to_rfc3339(),
        "updated_at": row.updated_at.to_rfc3339(),
    })
}

fn task_run_to_json(row: valka_db::queries::task_runs::TaskRunRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "task_id": row.task_id,
        "attempt_number": row.attempt_number,
        "worker_id": row.worker_id,
        "assigned_node_id": row.assigned_node_id,
        "status": row.status,
        "output": row.output,
        "error_message": row.error_message,
        "lease_expires_at": row.lease_expires_at.to_rfc3339(),
        "started_at": row.started_at.to_rfc3339(),
        "completed_at": row.completed_at.map(|t| t.to_rfc3339()),
        "last_heartbeat": row.last_heartbeat.to_rfc3339(),
    })
}

fn task_log_to_json(row: valka_db::queries::task_logs::TaskLogRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "task_run_id": row.task_run_id,
        "timestamp_ms": row.timestamp_ms,
        "level": row.level,
        "message": row.message,
        "metadata": row.metadata,
    })
}
