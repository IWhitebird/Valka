use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use valka_core::{MatchingConfig, NodeId, WorkerId};
use valka_db::queries::{task_runs, tasks};
use valka_dispatcher::DispatcherService;
use valka_dispatcher::worker_handle::WorkerHandle;
use valka_matching::MatchingService;
use valka_proto::WorkerResponse;

use super::helpers::*;

fn make_dispatcher(pool: PgPool) -> (DispatcherService, MatchingService) {
    let matching = MatchingService::new(MatchingConfig::default());
    let node_id = NodeId::new();
    let (event_tx, _) = broadcast::channel::<valka_proto::TaskEvent>(128);
    let (log_tx, _) = mpsc::channel::<valka_proto::LogEntry>(128);
    let dispatcher = DispatcherService::new(matching.clone(), pool, node_id, event_tx, log_tx);
    (dispatcher, matching)
}

fn make_worker_handle(concurrency: i32) -> (WorkerHandle, mpsc::Receiver<WorkerResponse>) {
    let (tx, rx) = mpsc::channel::<WorkerResponse>(16);
    let handle = WorkerHandle::new(
        WorkerId::new(),
        "test-worker".to_string(),
        vec!["default".to_string()],
        concurrency,
        tx,
        String::new(),
    );
    (handle, rx)
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_register_deregister(pool: PgPool) {
    let (dispatcher, _matching) = make_dispatcher(pool);
    let (handle, _rx) = make_worker_handle(1);
    let worker_id = handle.worker_id.clone();

    dispatcher.register_worker(handle).await;
    assert_eq!(dispatcher.workers().len(), 1);

    dispatcher.deregister_worker(&worker_id).await;
    assert_eq!(dispatcher.workers().len(), 0);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_handle_task_result_success(pool: PgPool) {
    let (task, run) = create_running_task(&pool, "demo").await;
    let (dispatcher, _matching) = make_dispatcher(pool.clone());

    let (handle, _rx) = make_worker_handle(2);
    let worker_id = handle.worker_id.clone();
    dispatcher.register_worker(handle).await;

    // Assign the task to the worker so complete_task removes it
    if let Some(mut h) = dispatcher.workers().get_mut(worker_id.as_ref()) {
        h.assign_task(task.id.clone());
    }

    let result = valka_proto::TaskResult {
        task_id: task.id.clone(),
        task_run_id: run.id.clone(),
        success: true,
        output: serde_json::json!({"done": true}).to_string(),
        error_message: String::new(),
        retryable: false,
    };
    dispatcher.handle_task_result(&worker_id, result).await;

    // Verify DB state
    let task_after = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(task_after.status, "COMPLETED");
    assert_eq!(task_after.output.unwrap()["done"], true);

    let run_after = task_runs::get_task_run(&pool, &run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run_after.status, "COMPLETED");
    assert!(run_after.completed_at.is_some());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_handle_task_result_failure_retryable(pool: PgPool) {
    let (task, run) = create_running_task(&pool, "demo").await;
    let (dispatcher, _matching) = make_dispatcher(pool.clone());

    let (handle, _rx) = make_worker_handle(2);
    let worker_id = handle.worker_id.clone();
    dispatcher.register_worker(handle).await;

    let result = valka_proto::TaskResult {
        task_id: task.id.clone(),
        task_run_id: run.id.clone(),
        success: false,
        output: String::new(),
        error_message: "timeout".to_string(),
        retryable: true,
    };
    dispatcher.handle_task_result(&worker_id, result).await;

    let task_after = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(task_after.status, "RETRY");

    let run_after = task_runs::get_task_run(&pool, &run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run_after.status, "FAILED");
    assert_eq!(run_after.error_message.as_deref(), Some("timeout"));
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_handle_task_result_failure_non_retryable(pool: PgPool) {
    let (task, run) = create_running_task(&pool, "demo").await;
    let (dispatcher, _matching) = make_dispatcher(pool.clone());

    let (handle, _rx) = make_worker_handle(2);
    let worker_id = handle.worker_id.clone();
    dispatcher.register_worker(handle).await;

    let result = valka_proto::TaskResult {
        task_id: task.id.clone(),
        task_run_id: run.id.clone(),
        success: false,
        output: String::new(),
        error_message: "fatal".to_string(),
        retryable: false,
    };
    dispatcher.handle_task_result(&worker_id, result).await;

    let task_after = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(task_after.status, "FAILED");
    assert_eq!(task_after.error_message.as_deref(), Some("fatal"));
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_cancel_task_on_worker(pool: PgPool) {
    let (dispatcher, _matching) = make_dispatcher(pool.clone());

    let (handle, mut rx) = make_worker_handle(2);
    let worker_id = handle.worker_id.clone();
    dispatcher.register_worker(handle).await;

    // Assign a task
    let task_id = "cancel-me";
    if let Some(mut h) = dispatcher.workers().get_mut(worker_id.as_ref()) {
        h.assign_task(task_id.to_string());
    }

    let cancelled = dispatcher.cancel_task_on_worker(task_id).await;
    assert!(cancelled, "Should find and cancel the task");

    // Verify cancellation message sent to worker
    let msg = rx.recv().await.unwrap();
    match msg.response.unwrap() {
        valka_proto::worker_response::Response::TaskCancellation(cancel) => {
            assert_eq!(cancel.task_id, task_id);
        }
        _ => panic!("Expected TaskCancellation"),
    }
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_cancel_nonexistent_task(pool: PgPool) {
    let (dispatcher, _matching) = make_dispatcher(pool);

    let cancelled = dispatcher.cancel_task_on_worker("nonexistent").await;
    assert!(!cancelled, "Should return false for nonexistent task");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_handle_heartbeat(pool: PgPool) {
    // Create task with short initial lease so heartbeat extends it
    let task = create_test_task(&pool, "q", "t").await;
    valka_db::queries::tasks::update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();
    let run = create_test_run(
        &pool,
        &task.id,
        1,
        chrono::Utc::now() + chrono::Duration::seconds(10),
    )
    .await;
    let task = valka_db::queries::tasks::get_task(&pool, &task.id)
        .await
        .unwrap()
        .unwrap();

    let (dispatcher, _matching) = make_dispatcher(pool.clone());

    let (handle, _rx) = make_worker_handle(2);
    let worker_id = handle.worker_id.clone();
    dispatcher.register_worker(handle).await;

    let heartbeat = valka_proto::Heartbeat {
        active_task_ids: vec![task.id.clone()],
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
    };
    dispatcher.handle_heartbeat(&worker_id, heartbeat).await;

    // Verify lease was extended
    let run_after = task_runs::get_task_run(&pool, &run.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        run_after.lease_expires_at > run.lease_expires_at,
        "Heartbeat should extend lease"
    );
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_emit_event(pool: PgPool) {
    let matching = MatchingService::new(MatchingConfig::default());
    let node_id = NodeId::new();
    let (event_tx, mut event_rx) = broadcast::channel::<valka_proto::TaskEvent>(128);
    let (log_tx, _) = mpsc::channel::<valka_proto::LogEntry>(128);
    let dispatcher =
        DispatcherService::new(matching, pool.clone(), node_id, event_tx.clone(), log_tx);

    let (task, run) = create_running_task(&pool, "q").await;
    let (handle, _rx) = make_worker_handle(2);
    let worker_id = handle.worker_id.clone();
    dispatcher.register_worker(handle).await;

    let result = valka_proto::TaskResult {
        task_id: task.id.clone(),
        task_run_id: run.id.clone(),
        success: true,
        output: String::new(),
        error_message: String::new(),
        retryable: false,
    };
    dispatcher.handle_task_result(&worker_id, result).await;

    // Should have emitted a COMPLETED event
    let event = event_rx.recv().await.unwrap();
    assert_eq!(event.task_id, task.id);
    assert_eq!(event.new_status, 4); // COMPLETED
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_handle_log_batch(pool: PgPool) {
    let matching = MatchingService::new(MatchingConfig::default());
    let node_id = NodeId::new();
    let (event_tx, _) = broadcast::channel::<valka_proto::TaskEvent>(128);
    let (log_tx, mut log_rx) = mpsc::channel::<valka_proto::LogEntry>(128);
    let dispatcher = DispatcherService::new(matching, pool, node_id, event_tx, log_tx);

    let (handle, _rx) = make_worker_handle(1);
    let worker_id = handle.worker_id.clone();
    dispatcher.register_worker(handle).await;

    let batch = valka_proto::LogBatch {
        entries: vec![valka_proto::LogEntry {
            task_run_id: "run-1".to_string(),
            timestamp_ms: 1000,
            level: 2,
            message: "hello".to_string(),
            metadata: String::new(),
        }],
    };
    dispatcher.handle_log_batch(&worker_id, batch).await;

    let entry = log_rx.recv().await.unwrap();
    assert_eq!(entry.message, "hello");
    assert_eq!(entry.task_run_id, "run-1");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dispatcher_multiple_workers(pool: PgPool) {
    let (dispatcher, _matching) = make_dispatcher(pool);

    let (h1, _rx1) = make_worker_handle(2);
    let (h2, _rx2) = make_worker_handle(3);
    let id1 = h1.worker_id.clone();
    let id2 = h2.worker_id.clone();

    dispatcher.register_worker(h1).await;
    dispatcher.register_worker(h2).await;
    assert_eq!(dispatcher.workers().len(), 2);

    dispatcher.deregister_worker(&id1).await;
    assert_eq!(dispatcher.workers().len(), 1);
    assert!(dispatcher.workers().contains_key(id2.as_ref()));
}
