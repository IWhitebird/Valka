use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use valka_cluster::{ClusterManager, NodeForwarder};
use valka_core::{MatchingConfig, NodeId, TaskId, partition_for_task};
use valka_db::queries::task_runs::{CreateTaskRunParams, TaskRunRow};
use valka_db::queries::tasks::{CreateTaskParams, TaskRow};
use valka_dispatcher::DispatcherService;
use valka_matching::MatchingService;

/// Create a task with sensible defaults. Returns the inserted TaskRow.
pub async fn create_test_task(pool: &PgPool, queue: &str, name: &str) -> TaskRow {
    let id = TaskId::new().0;
    let partition = partition_for_task(queue, &id, 4);
    valka_db::queries::tasks::create_task(
        pool,
        CreateTaskParams {
            id,
            queue_name: queue.to_string(),
            task_name: name.to_string(),
            partition_id: partition.0,
            input: Some(serde_json::json!({"key": "value"})),
            priority: 0,
            max_retries: 3,
            timeout_seconds: 300,
            idempotency_key: None,
            metadata: serde_json::json!({}),
            scheduled_at: None,
        },
    )
    .await
    .expect("create_test_task failed")
}

/// Create a task with all fields customizable.
pub async fn create_test_task_full(pool: &PgPool, params: CreateTaskParams) -> TaskRow {
    valka_db::queries::tasks::create_task(pool, params)
        .await
        .expect("create_test_task_full failed")
}

/// Create a task + running task_run. Returns (TaskRow, TaskRunRow).
pub async fn create_running_task(pool: &PgPool, queue: &str) -> (TaskRow, TaskRunRow) {
    let task = create_test_task(pool, queue, "running-task").await;

    // Update to RUNNING
    valka_db::queries::tasks::update_task_status(pool, &task.id, "RUNNING")
        .await
        .unwrap();

    let run = valka_db::queries::task_runs::create_task_run(
        pool,
        CreateTaskRunParams {
            id: uuid::Uuid::now_v7().to_string(),
            task_id: task.id.clone(),
            attempt_number: 1,
            worker_id: uuid::Uuid::now_v7().to_string(),
            assigned_node_id: uuid::Uuid::now_v7().to_string(),
            lease_expires_at: Utc::now() + Duration::seconds(330),
        },
    )
    .await
    .expect("create_task_run failed");

    // Re-fetch task to get updated status
    let task = valka_db::queries::tasks::get_task(pool, &task.id)
        .await
        .unwrap()
        .unwrap();

    (task, run)
}

/// Build the axum REST router wired to a real PG pool + in-memory services.
pub fn build_test_router(pool: PgPool) -> Router {
    let matching = MatchingService::new(MatchingConfig::default());
    let node_id = NodeId::new();
    let (event_tx, _) = broadcast::channel::<valka_proto::TaskEvent>(128);
    let (log_tx, _log_rx) = mpsc::channel::<valka_proto::LogEntry>(128);

    let dispatcher = DispatcherService::new(
        matching.clone(),
        pool.clone(),
        node_id.clone(),
        event_tx.clone(),
        log_tx,
    );

    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .build_recorder()
        .handle();

    let cluster = Arc::new(ClusterManager::new_single_node(
        node_id,
        matching.config().num_partitions as i32,
    ));
    let forwarder = NodeForwarder::new();

    valka_server::rest::build_api_router(
        pool,
        event_tx,
        matching,
        dispatcher,
        metrics_handle,
        cluster,
        forwarder,
    )
}

/// Convert a serde_json::Value into an axum-compatible request body.
pub fn json_body(value: serde_json::Value) -> String {
    serde_json::to_string(&value).unwrap()
}

/// Read the response body as bytes and parse as JSON.
pub async fn parse_response_json(
    response: axum::http::Response<axum::body::Body>,
) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// Assert that a response is a JSON error with the expected status, code, and substring.
pub async fn assert_error_response(
    response: axum::http::Response<axum::body::Body>,
    expected_status: StatusCode,
    expected_code: &str,
    message_contains: &str,
) {
    assert_eq!(response.status(), expected_status);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("Error response should be valid JSON");
    assert_eq!(
        json["code"].as_str().unwrap(),
        expected_code,
        "Expected error code {expected_code}, got {:?}",
        json["code"]
    );
    let error_msg = json["error"].as_str().unwrap();
    assert!(
        error_msg.contains(message_contains),
        "Expected error message to contain '{message_contains}', got '{error_msg}'"
    );
}

/// Helper to create a CreateTaskParams with defaults.
pub fn default_task_params(queue: &str, name: &str) -> CreateTaskParams {
    let id = TaskId::new().0;
    let partition = partition_for_task(queue, &id, 4);
    CreateTaskParams {
        id,
        queue_name: queue.to_string(),
        task_name: name.to_string(),
        partition_id: partition.0,
        input: Some(serde_json::json!({"key": "value"})),
        priority: 0,
        max_retries: 3,
        timeout_seconds: 300,
        idempotency_key: None,
        metadata: serde_json::json!({}),
        scheduled_at: None,
    }
}

/// Shorthand: create a task run for an existing task.
pub async fn create_test_run(
    pool: &PgPool,
    task_id: &str,
    attempt: i32,
    lease_expires_at: DateTime<Utc>,
) -> TaskRunRow {
    valka_db::queries::task_runs::create_task_run(
        pool,
        CreateTaskRunParams {
            id: uuid::Uuid::now_v7().to_string(),
            task_id: task_id.to_string(),
            attempt_number: attempt,
            worker_id: uuid::Uuid::now_v7().to_string(),
            assigned_node_id: uuid::Uuid::now_v7().to_string(),
            lease_expires_at,
        },
    )
    .await
    .expect("create_test_run failed")
}
