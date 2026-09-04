use chrono::Utc;
use tokio::sync::mpsc;
use valka_core::{MatchingConfig, NodeId, PartitionId, WorkerId};
use valka_dispatcher::DispatcherService;
use valka_dispatcher::worker_handle::WorkerHandle;
use valka_engine::{Engine, EngineConfig};
use valka_matching::MatchingService;
use valka_proto::WorkerResponse;
use valka_wal::Store;

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

async fn make_engine() -> Engine {
    Engine::open(Store::memory(), EngineConfig::for_tests("unit"))
        .await
        .unwrap()
}

async fn make_dispatcher() -> DispatcherService {
    let matching = MatchingService::new(MatchingConfig::default());
    let engine = make_engine().await;
    let node_id = NodeId::new();
    let (log_tx, _) = mpsc::channel(64);
    DispatcherService::new(matching, engine, node_id, log_tx)
}

#[tokio::test]
async fn test_dispatcher_register_deregister() {
    let dispatcher = make_dispatcher().await;
    let worker_id = WorkerId::new();
    let (handle, _rx) = make_handle_with_id(worker_id.clone(), 2);

    dispatcher.register_worker(handle).await;
    assert_eq!(dispatcher.workers().len(), 1);

    dispatcher.deregister_worker(&worker_id).await;
    assert_eq!(dispatcher.workers().len(), 0);
}

#[tokio::test]
async fn test_dispatcher_multiple_workers() {
    let dispatcher = make_dispatcher().await;

    for _ in 0..3 {
        let (handle, _rx) = make_handle_with_id(WorkerId::new(), 1);
        dispatcher.register_worker(handle).await;
    }

    assert_eq!(dispatcher.workers().len(), 3);
}

#[tokio::test]
async fn test_engine_events_flow_through_dispatcher_engine() {
    let dispatcher = make_dispatcher().await;
    let mut rx = dispatcher.engine().subscribe();
    let task = dispatcher
        .engine()
        .create_task(valka_engine::CreateTask {
            queue_name: "demo".into(),
            task_name: "t".into(),
            input: None,
            priority: 0,
            max_retries: 3,
            timeout_seconds: 30,
            idempotency_key: None,
            metadata: serde_json::json!({}),
            scheduled_at: None,
        })
        .await
        .unwrap();
    let ev = rx.recv().await.unwrap();
    assert_eq!(ev.task_id, task.id);
    assert_eq!(ev.new, Some(valka_core::TaskStatus::Pending));
}

#[tokio::test]
async fn test_dispatcher_cancel_nonexistent_task() {
    let dispatcher = make_dispatcher().await;
    let result = dispatcher.cancel_task_on_worker("nonexistent-task").await;
    assert!(!result, "Should return false when no worker has the task");
}

#[tokio::test]
async fn test_dispatcher_cancel_active_task() {
    let dispatcher = make_dispatcher().await;
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

// ─── Signal routing tests ───────────────────────────────────────────

#[tokio::test]
async fn test_dispatcher_send_signal_to_active_task() {
    let dispatcher = make_dispatcher().await;
    let worker_id = WorkerId::new();
    let (mut handle, mut rx) = make_handle_with_id(worker_id.clone(), 2);
    handle.assign_task("task-signaled".to_string());
    dispatcher.register_worker(handle).await;

    let signal = valka_proto::TaskSignal {
        signal_id: "sig-1".to_string(),
        task_id: "task-signaled".to_string(),
        signal_name: "approve".to_string(),
        payload: r#"{"ok": true}"#.to_string(),
        timestamp_ms: 1700000000000,
    };

    let delivered = dispatcher
        .send_signal_to_worker("task-signaled", signal)
        .await;
    assert!(
        delivered,
        "Should find and deliver signal to worker with active task"
    );

    let msg = rx.recv().await.expect("Should receive signal");
    match msg.response {
        Some(valka_proto::worker_response::Response::TaskSignal(s)) => {
            assert_eq!(s.signal_id, "sig-1");
            assert_eq!(s.task_id, "task-signaled");
            assert_eq!(s.signal_name, "approve");
            assert_eq!(s.payload, r#"{"ok": true}"#);
        }
        other => panic!("Expected TaskSignal, got {other:?}"),
    }
}

#[tokio::test]
async fn test_dispatcher_send_signal_no_worker() {
    let dispatcher = make_dispatcher().await;

    let signal = valka_proto::TaskSignal {
        signal_id: "sig-orphan".to_string(),
        task_id: "no-such-task".to_string(),
        signal_name: "ping".to_string(),
        payload: String::new(),
        timestamp_ms: 0,
    };

    let delivered = dispatcher
        .send_signal_to_worker("no-such-task", signal)
        .await;
    assert!(!delivered, "Should return false when no worker registered");
}

#[tokio::test]
async fn test_dispatcher_send_signal_wrong_task() {
    let dispatcher = make_dispatcher().await;
    let worker_id = WorkerId::new();
    let (mut handle, _rx) = make_handle_with_id(worker_id.clone(), 2);
    handle.assign_task("task-A".to_string());
    dispatcher.register_worker(handle).await;

    let signal = valka_proto::TaskSignal {
        signal_id: "sig-wrong".to_string(),
        task_id: "task-B".to_string(),
        signal_name: "notify".to_string(),
        payload: String::new(),
        timestamp_ms: 0,
    };

    let delivered = dispatcher.send_signal_to_worker("task-B", signal).await;
    assert!(
        !delivered,
        "Should return false when worker has different task"
    );
}

// ─── Additional WorkerHandle edge cases ─────────────────────────────

#[test]
fn test_worker_at_capacity_zero_slots() {
    let (mut handle, _rx) = make_handle_with_id(WorkerId::new(), 1);
    handle.assign_task("task-1".to_string());
    assert_eq!(handle.available_slots(), 0, "Should have zero slots");
    assert!(!handle.is_idle());
}

#[test]
fn test_worker_assign_beyond_capacity() {
    let (mut handle, _rx) = make_handle_with_id(WorkerId::new(), 2);
    handle.assign_task("t1".to_string());
    handle.assign_task("t2".to_string());
    handle.assign_task("t3".to_string());
    // available_slots goes negative — enforcement is in the match loop, not the handle
    assert_eq!(handle.available_slots(), -1);
}

#[tokio::test]
async fn test_handle_task_result_empty_output() {
    let dispatcher = make_dispatcher().await;
    let worker_id = WorkerId::new();
    let (mut handle, _rx) = make_handle_with_id(worker_id.clone(), 2);
    handle.assign_task("task-empty".to_string());
    dispatcher.register_worker(handle).await;

    // Verify the task is assigned
    {
        let h = dispatcher.workers().get(worker_id.as_ref()).unwrap();
        assert!(h.active_tasks.contains("task-empty"));
    }

    // handle_task_result requires DB — but we can test worker state cleanup via
    // complete_task on the handle directly
    if let Some(mut h) = dispatcher.workers().get_mut(worker_id.as_ref()) {
        h.complete_task("task-empty");
    }

    let h = dispatcher.workers().get(worker_id.as_ref()).unwrap();
    assert!(
        !h.active_tasks.contains("task-empty"),
        "Task should be removed"
    );
    assert_eq!(h.available_slots(), 2);
}

#[tokio::test]
async fn test_send_signal_worker_channel_closed() {
    let dispatcher = make_dispatcher().await;
    let worker_id = WorkerId::new();
    let (mut handle, rx) = make_handle_with_id(worker_id.clone(), 2);
    handle.assign_task("task-orphan".to_string());
    dispatcher.register_worker(handle).await;

    // Drop the receiver — simulates worker disconnect
    drop(rx);

    let signal = valka_proto::TaskSignal {
        signal_id: "sig-closed".to_string(),
        task_id: "task-orphan".to_string(),
        signal_name: "ping".to_string(),
        payload: String::new(),
        timestamp_ms: 0,
    };

    // send_signal_to_worker finds the task but send may silently fail
    let delivered = dispatcher
        .send_signal_to_worker("task-orphan", signal)
        .await;
    assert!(delivered, "Should return true (found the task)");
    // No panic despite closed channel
}

#[tokio::test]
async fn test_cancel_worker_channel_closed() {
    let dispatcher = make_dispatcher().await;
    let worker_id = WorkerId::new();
    let (mut handle, rx) = make_handle_with_id(worker_id.clone(), 2);
    handle.assign_task("task-cancel-closed".to_string());
    dispatcher.register_worker(handle).await;

    // Drop the receiver
    drop(rx);

    let result = dispatcher.cancel_task_on_worker("task-cancel-closed").await;
    assert!(result, "Should return true (found the task)");
    // No panic despite closed channel
}

#[tokio::test]
async fn test_register_same_worker_id_twice() {
    let dispatcher = make_dispatcher().await;
    let worker_id = WorkerId::new();

    let (h1, _rx1) = make_handle_with_id(worker_id.clone(), 1);
    let (h2, _rx2) = make_handle_with_id(worker_id.clone(), 3);

    dispatcher.register_worker(h1).await;
    dispatcher.register_worker(h2).await;

    // DashMap overwrites — only 1 entry
    assert_eq!(dispatcher.workers().len(), 1);
    // Should have the latest concurrency
    let h = dispatcher.workers().get(worker_id.as_ref()).unwrap();
    assert_eq!(h.concurrency, 3);
}

#[tokio::test]
async fn test_deregister_nonexistent_worker() {
    let dispatcher = make_dispatcher().await;
    // Should not panic
    dispatcher.deregister_worker(&WorkerId::new()).await;
    assert_eq!(dispatcher.workers().len(), 0);
}

#[tokio::test]
async fn test_dispatcher_clone_shares_workers() {
    let dispatcher_a = make_dispatcher().await;
    let dispatcher_b = dispatcher_a.clone();

    let (handle, _rx) = make_handle_with_id(WorkerId::new(), 1);
    dispatcher_a.register_worker(handle).await;

    // Clone B should see the worker registered on A
    assert_eq!(
        dispatcher_b.workers().len(),
        1,
        "Clone should share workers via Arc"
    );
}

#[tokio::test]
async fn test_deregister_clears_matching_service() {
    let matching = MatchingService::new(MatchingConfig::default());
    let engine = make_engine().await;
    let node_id = NodeId::new();
    let (log_tx, _) = mpsc::channel(64);
    let dispatcher = DispatcherService::new(matching.clone(), engine, node_id, log_tx);

    let worker_id = WorkerId::new();
    let (handle, _rx) = make_handle_with_id(worker_id.clone(), 2);
    dispatcher.register_worker(handle).await;

    // Manually register worker in matching service (like the stream handler does)
    matching.ensure_queue("default");
    let _mrx = matching.register_worker("default", PartitionId(0), worker_id.clone());

    // Deregister — should remove from both dispatcher AND matching service
    dispatcher.deregister_worker(&worker_id).await;
    assert_eq!(dispatcher.workers().len(), 0);

    // Offer task — should fail because worker is gone from matching too
    let envelope = valka_matching::partition::TaskEnvelope {
        task_id: "t1".to_string(),
        task_run_id: String::new(),
        queue_name: "default".to_string(),
        task_name: "test".to_string(),
        input: None,
        attempt_number: 1,
        timeout_seconds: 300,
        metadata: "{}".to_string(),
        priority: 0,
    };
    let result = matching.offer_task("default", PartitionId(0), envelope);
    assert!(
        result.is_err(),
        "Worker should be gone from matching service"
    );
}
