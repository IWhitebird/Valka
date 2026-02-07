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
use tokio::sync::broadcast;
use tracing::info;

use valka_core::{TaskId, partition_for_task};
use valka_db::DbPool;
use valka_matching::MatchingService;

#[derive(Clone)]
struct AppState {
    pool: DbPool,
    event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    matching: MatchingService,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
}

pub async fn serve_rest(
    addr: SocketAddr,
    pool: DbPool,
    event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    matching: MatchingService,
    metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
) -> Result<(), anyhow::Error> {
    let state = AppState {
        pool,
        event_tx,
        matching,
        metrics_handle,
    };

    let app = Router::new()
        .route("/api/v1/tasks", post(create_task).get(list_tasks))
        .route("/api/v1/tasks/{task_id}", get(get_task))
        .route("/api/v1/tasks/{task_id}/cancel", post(cancel_task))
        .route("/api/v1/events", get(subscribe_events_sse))
        .route("/metrics", get(metrics))
        .route("/healthz", get(healthz))
        .with_state(state);

    info!("REST server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

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
            metadata: body.metadata.unwrap_or(serde_json::json!({})),
            scheduled_at,
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    valka_core::metrics::record_task_created(&body.queue_name);

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
    let task = valka_db::queries::tasks::cancel_task(&state.pool, &task_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Task not found or not in cancellable state".to_string(),
            )
        })?;

    Ok(Json(task_row_to_json(task)))
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
        "scheduled_at": row.scheduled_at.map(|t| t.to_rfc3339()),
        "created_at": row.created_at.to_rfc3339(),
        "updated_at": row.updated_at.to_rfc3339(),
    })
}
