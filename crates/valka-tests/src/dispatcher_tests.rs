use chrono::Utc;
use tokio::sync::{broadcast, mpsc};
use valka_core::{MatchingConfig, NodeId, WorkerId};
use valka_db::DbPool;
use valka_dispatcher::DispatcherService;
use valka_dispatcher::worker_handle::WorkerHandle;
use valka_matching::MatchingService;
use valka_proto::WorkerResponse;

fn make_handle_with_id(
    worker_id: WorkerId,
    concurrency: i32,
) -> (WorkerHandle, mpsc::Receiver<WorkerResponse>) {
    let (tx, rx) = mpsc::channel::<WorkerResponse>(64);
    let handle = WorkerHandle::new(
        worker_id,
        "test-worker".to_string(),
        vec!["default".to_string()],
        concurrency,
        tx,
        String::new(),
    );
    (handle, rx)
}

// === WorkerHandle tests ===

#[test]
fn test_worker_handle_available_slots() {
    let (mut handle, _rx) = make_handle_with_id(WorkerId::new(), 3);
    assert_eq!(handle.available_slots(), 3);

    handle.assign_task("task-1".to_string());
    assert_eq!(handle.available_slots(), 2);

    handle.assign_task("task-2".to_string());
    assert_eq!(handle.available_slots(), 1);
}

#[test]
fn test_worker_handle_assign_and_complete() {
    let (mut handle, _rx) = make_handle_with_id(WorkerId::new(), 2);

    handle.assign_task("task-1".to_string());
    assert_eq!(handle.available_slots(), 1);

    handle.complete_task("task-1");
    assert_eq!(handle.available_slots(), 2);
}

#[test]
fn test_worker_handle_zero_concurrency() {
    let (handle, _rx) = make_handle_with_id(WorkerId::new(), 0);
    assert_eq!(handle.available_slots(), 0);
}

#[test]
fn test_worker_handle_is_idle() {
    let (mut handle, _rx) = make_handle_with_id(WorkerId::new(), 2);
    assert!(handle.is_idle());

    handle.assign_task("task-1".to_string());
    assert!(!handle.is_idle());

    handle.complete_task("task-1");
    assert!(handle.is_idle());
}

#[test]
fn test_worker_handle_complete_unknown_task() {
    let (mut handle, _rx) = make_handle_with_id(WorkerId::new(), 2);
    // Should not panic when completing a task that was never assigned
    handle.complete_task("unknown-task");
    assert_eq!(handle.available_slots(), 2);
}

#[test]
fn test_worker_handle_duplicate_assign() {
    let (mut handle, _rx) = make_handle_with_id(WorkerId::new(), 5);
    handle.assign_task("task-1".to_string());
    // HashSet: inserting same value is idempotent
    handle.assign_task("task-1".to_string());
    assert_eq!(handle.active_tasks.len(), 1);
    assert_eq!(handle.available_slots(), 4);
}

#[test]
fn test_worker_handle_heartbeat_updates_timestamp() {
    let (mut handle, _rx) = make_handle_with_id(WorkerId::new(), 1);
    let before = handle.last_heartbeat;

    // Small sleep to ensure time changes
    std::thread::sleep(std::time::Duration::from_millis(10));

    handle.update_heartbeat();
    assert!(
        handle.last_heartbeat > before,
        "Heartbeat timestamp should be updated"
    );
}

#[test]
fn test_worker_handle_connected_at_set() {
    let now_before = Utc::now();
    let (handle, _rx) = make_handle_with_id(WorkerId::new(), 1);
    let now_after = Utc::now();

    assert!(handle.connected_at >= now_before);
    assert!(handle.connected_at <= now_after);
}

// === DispatcherService tests ===

fn make_pool() -> DbPool {
    // connect_lazy doesn't actually connect — safe for tests that don't hit the DB
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgresql://fake:fake@localhost:5432/fake")
        .unwrap()
}

fn make_dispatcher() -> DispatcherService {
    let matching = MatchingService::new(MatchingConfig::default());
    let pool = make_pool();
    let node_id = NodeId::new();
    let (event_tx, _) = broadcast::channel(64);
    let (log_tx, _) = mpsc::channel(64);
    DispatcherService::new(matching, pool, node_id, event_tx, log_tx)
}

#[tokio::test]
async fn test_dispatcher_register_deregister() {
    let dispatcher = make_dispatcher();
    let worker_id = WorkerId::new();
    let (handle, _rx) = make_handle_with_id(worker_id.clone(), 2);

    dispatcher.register_worker(handle).await;
    assert_eq!(dispatcher.workers().len(), 1);

    dispatcher.deregister_worker(&worker_id).await;
    assert_eq!(dispatcher.workers().len(), 0);
}

#[tokio::test]
async fn test_dispatcher_multiple_workers() {
    let dispatcher = make_dispatcher();

    for _ in 0..3 {
        let (handle, _rx) = make_handle_with_id(WorkerId::new(), 1);
        dispatcher.register_worker(handle).await;
    }

    assert_eq!(dispatcher.workers().len(), 3);
}

#[tokio::test]
async fn test_dispatcher_emit_event() {
    let matching = MatchingService::new(MatchingConfig::default());
    let pool = make_pool();
    let node_id = NodeId::new();
    let (event_tx, _) = broadcast::channel(64);
    let (log_tx, _) = mpsc::channel(64);
    let dispatcher = DispatcherService::new(matching, pool, node_id, event_tx, log_tx);

    // Subscribe before emitting
    let mut event_rx = dispatcher.event_tx().subscribe();

    // We can't call emit_event directly (private), but we can verify event_tx works
    let event = valka_proto::TaskEvent {
        event_id: "test-event".to_string(),
        task_id: "task-1".to_string(),
        queue_name: "demo".to_string(),
        previous_status: 0,
        new_status: 3,
        worker_id: String::new(),
        node_id: String::new(),
        attempt_number: 0,
        error_message: String::new(),
        timestamp_ms: 0,
    };
    dispatcher.event_tx().send(event.clone()).unwrap();

    let received = event_rx.recv().await.unwrap();
    assert_eq!(received.task_id, "task-1");
    assert_eq!(received.new_status, 3);
}

#[tokio::test]
async fn test_dispatcher_cancel_nonexistent_task() {
    let dispatcher = make_dispatcher();
    let result = dispatcher.cancel_task_on_worker("nonexistent-task").await;
    assert!(!result, "Should return false when no worker has the task");
}

#[tokio::test]
async fn test_dispatcher_cancel_active_task() {
    let dispatcher = make_dispatcher();
    let worker_id = WorkerId::new();
    let (mut handle, mut rx) = make_handle_with_id(worker_id.clone(), 2);
    handle.assign_task("task-to-cancel".to_string());
    dispatcher.register_worker(handle).await;

    let result = dispatcher.cancel_task_on_worker("task-to-cancel").await;
    assert!(result, "Should find and cancel the task");

    // Verify the cancel message was sent
    let msg = rx.recv().await.expect("Should receive cancellation");
    match msg.response {
        Some(valka_proto::worker_response::Response::TaskCancellation(cancel)) => {
            assert_eq!(cancel.task_id, "task-to-cancel");
            assert_eq!(cancel.reason, "Cancelled by user");
        }
        other => panic!("Expected TaskCancellation, got {other:?}"),
    }
}
