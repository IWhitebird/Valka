use valka_core::{MatchingConfig, PartitionId, WorkerId};
use valka_matching::MatchingService;
use valka_matching::partition::TaskEnvelope;

fn make_envelope(task_id: &str, queue: &str) -> TaskEnvelope {
    TaskEnvelope {
        task_id: task_id.to_string(),
        task_run_id: String::new(),
        queue_name: queue.to_string(),
        task_name: "test_task".to_string(),
        input: Some(r#"{"key": "value"}"#.to_string()),
        attempt_number: 1,
        timeout_seconds: 300,
        metadata: "{}".to_string(),
        priority: 0,
    }
}

#[tokio::test]
async fn test_sync_match_with_waiting_worker() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    let queue = "test.queue";
    service.ensure_queue(queue);

    let worker_id = WorkerId::new();
    let rx = service.register_worker(queue, PartitionId(0), worker_id.clone());

    let envelope = make_envelope("task-1", queue);
    let result = service.offer_task(queue, PartitionId(0), envelope);
    assert!(result.is_ok(), "Task should be matched with waiting worker");

    let received = rx.await.expect("Should receive task");
    assert_eq!(received.task_id, "task-1");
}

#[tokio::test]
async fn test_no_match_without_workers() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    let queue = "test.queue";
    service.ensure_queue(queue);

    let envelope = make_envelope("task-1", queue);
    let result = service.offer_task(queue, PartitionId(0), envelope);
    assert!(
        result.is_err(),
        "Task should not be matched without workers"
    );
}

#[tokio::test]
async fn test_buffer_task_and_match_on_worker_register() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    let queue = "test.queue";
    service.ensure_queue(queue);

    let envelope = make_envelope("task-1", queue);
    let buffered = service.buffer_task(queue, PartitionId(0), envelope);
    assert!(buffered, "Task should be buffered");

    let worker_id = WorkerId::new();
    let rx = service.register_worker(queue, PartitionId(0), worker_id);

    let received = rx.await.expect("Worker should receive buffered task");
    assert_eq!(received.task_id, "task-1");
}

#[tokio::test]
async fn test_deregister_worker() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    let queue = "test.queue";
    service.ensure_queue(queue);

    let worker_id = WorkerId::new();
    let _rx = service.register_worker(queue, PartitionId(0), worker_id.clone());

    service.deregister_worker(&worker_id);

    let envelope = make_envelope("task-1", queue);
    let result = service.offer_task(queue, PartitionId(0), envelope);
    assert!(
        result.is_err(),
        "Task should not match after worker deregistered"
    );
}

#[tokio::test]
async fn test_multiple_workers_round_robin() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    let queue = "test.queue";
    service.ensure_queue(queue);

    let w1 = WorkerId::new();
    let w2 = WorkerId::new();
    let rx1 = service.register_worker(queue, PartitionId(0), w1);
    let rx2 = service.register_worker(queue, PartitionId(0), w2);

    let e1 = make_envelope("task-1", queue);
    assert!(service.offer_task(queue, PartitionId(0), e1).is_ok());
    let received1 = rx1.await.expect("Worker 1 should receive task");
    assert_eq!(received1.task_id, "task-1");

    let e2 = make_envelope("task-2", queue);
    assert!(service.offer_task(queue, PartitionId(0), e2).is_ok());
    let received2 = rx2.await.expect("Worker 2 should receive task");
    assert_eq!(received2.task_id, "task-2");
}

#[tokio::test]
async fn test_partition_tree_forwarding() {
    let mut config = MatchingConfig::default();
    config.num_partitions = 4;
    config.branching_factor = 2;
    let service = MatchingService::new(config);

    let queue = "test.queue";
    service.ensure_queue(queue);

    let worker_id = WorkerId::new();
    let rx = service.register_worker(queue, PartitionId(0), worker_id);

    let envelope = make_envelope("task-1", queue);
    let result = service.offer_task(queue, PartitionId(1), envelope);
    assert!(result.is_ok(), "Task should be matched via tree forwarding");

    let received = rx.await.expect("Worker should receive forwarded task");
    assert_eq!(received.task_id, "task-1");
}

#[tokio::test]
async fn test_buffer_overflow() {
    let mut config = MatchingConfig::default();
    config.max_buffer_per_partition = 2;
    let service = MatchingService::new(config);

    let queue = "test.queue";
    service.ensure_queue(queue);

    assert!(service.buffer_task(queue, PartitionId(0), make_envelope("t1", queue)));
    assert!(service.buffer_task(queue, PartitionId(0), make_envelope("t2", queue)));
    assert!(
        !service.buffer_task(queue, PartitionId(0), make_envelope("t3", queue)),
        "Buffer should be full"
    );
}

#[tokio::test]
async fn test_offer_task_nonexistent_queue() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    // Don't call ensure_queue — offer_task calls it internally
    let envelope = make_envelope("task-1", "brand-new-queue");
    let result = service.offer_task("brand-new-queue", PartitionId(0), envelope);
    // No workers registered, so should fail
    assert!(result.is_err(), "Should fail with no workers on new queue");
}

#[tokio::test]
async fn test_multiple_queues_isolated() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    service.ensure_queue("queue-a");
    service.ensure_queue("queue-b");

    // Register worker on queue-a
    let worker_id = WorkerId::new();
    let rx = service.register_worker("queue-a", PartitionId(0), worker_id);

    // Offer task on queue-b — should NOT match the worker on queue-a
    let envelope = make_envelope("task-1", "queue-b");
    let result = service.offer_task("queue-b", PartitionId(0), envelope);
    assert!(
        result.is_err(),
        "Task on queue-b should not match worker on queue-a"
    );

    // Now offer on queue-a — should match
    let envelope2 = make_envelope("task-2", "queue-a");
    let result2 = service.offer_task("queue-a", PartitionId(0), envelope2);
    assert!(result2.is_ok());

    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "task-2");
}

#[tokio::test]
async fn test_worker_receiver_dropped_reclaims_task() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    let queue = "test.queue";
    service.ensure_queue(queue);

    // Register worker then drop the receiver
    let worker_id = WorkerId::new();
    let rx = service.register_worker(queue, PartitionId(0), worker_id);
    drop(rx); // Simulate worker disconnect

    // Offer task — the stale worker slot should be skipped
    let envelope = make_envelope("task-1", queue);
    let result = service.offer_task(queue, PartitionId(0), envelope);
    // Should fail because the only worker's receiver was dropped
    assert!(
        result.is_err(),
        "Should fail when worker receiver is dropped"
    );
}

#[tokio::test]
async fn test_buffer_then_multiple_workers() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    let queue = "test.queue";
    service.ensure_queue(queue);

    // Buffer 3 tasks
    assert!(service.buffer_task(queue, PartitionId(0), make_envelope("t1", queue)));
    assert!(service.buffer_task(queue, PartitionId(0), make_envelope("t2", queue)));
    assert!(service.buffer_task(queue, PartitionId(0), make_envelope("t3", queue)));

    // Register 3 workers — each should get one buffered task
    let rx1 = service.register_worker(queue, PartitionId(0), WorkerId::new());
    let rx2 = service.register_worker(queue, PartitionId(0), WorkerId::new());
    let rx3 = service.register_worker(queue, PartitionId(0), WorkerId::new());

    let r1 = rx1.await.unwrap();
    let r2 = rx2.await.unwrap();
    let r3 = rx3.await.unwrap();

    let mut ids: Vec<String> = vec![r1.task_id, r2.task_id, r3.task_id];
    ids.sort();
    assert_eq!(ids, vec!["t1", "t2", "t3"]);
}

#[tokio::test]
async fn test_tree_forwarding_deep_4_levels() {
    let mut config = MatchingConfig::default();
    config.num_partitions = 8;
    config.branching_factor = 2;
    let service = MatchingService::new(config);

    let queue = "deep.queue";
    service.ensure_queue(queue);

    // Register worker on partition 0 (root)
    let worker_id = WorkerId::new();
    let rx = service.register_worker(queue, PartitionId(0), worker_id);

    // Offer task on partition 7 (deepest leaf) — should forward up to root
    let envelope = make_envelope("task-deep", queue);
    let result = service.offer_task(queue, PartitionId(7), envelope);
    assert!(result.is_ok(), "Should match via deep tree forwarding");

    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "task-deep");
}

#[tokio::test]
async fn test_tree_forwarding_no_match_returns_err() {
    let mut config = MatchingConfig::default();
    config.num_partitions = 4;
    config.branching_factor = 2;
    let service = MatchingService::new(config);

    let queue = "empty.queue";
    service.ensure_queue(queue);

    // No workers registered anywhere
    let envelope = make_envelope("orphan-task", queue);
    let result = service.offer_task(queue, PartitionId(3), envelope);
    assert!(
        result.is_err(),
        "Should return Err when no workers on any partition"
    );

    let returned = result.unwrap_err();
    assert_eq!(returned.task_id, "orphan-task");
}

#[tokio::test]
async fn test_deregister_removes_from_all_partitions() {
    let mut config = MatchingConfig::default();
    config.num_partitions = 4;
    let service = MatchingService::new(config);

    let queue = "multi.queue";
    service.ensure_queue(queue);

    let worker_id = WorkerId::new();

    // Register same worker on partitions 0, 1, 2
    let _rx0 = service.register_worker(queue, PartitionId(0), worker_id.clone());
    let _rx1 = service.register_worker(queue, PartitionId(1), worker_id.clone());
    let _rx2 = service.register_worker(queue, PartitionId(2), worker_id.clone());

    // Deregister
    service.deregister_worker(&worker_id);

    // Offer on each partition — none should match
    for pid in 0..3 {
        let envelope = make_envelope(&format!("t-{pid}"), queue);
        let result = service.offer_task(queue, PartitionId(pid), envelope);
        assert!(
            result.is_err(),
            "Partition {pid} should have no workers after deregister"
        );
    }
}

#[tokio::test]
async fn test_single_partition_no_forwarding() {
    let mut config = MatchingConfig::default();
    config.num_partitions = 1;
    let service = MatchingService::new(config);

    let queue = "single.queue";
    service.ensure_queue(queue);

    let worker_id = WorkerId::new();
    let rx = service.register_worker(queue, PartitionId(0), worker_id);

    let envelope = make_envelope("task-solo", queue);
    let result = service.offer_task(queue, PartitionId(0), envelope);
    assert!(result.is_ok());

    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "task-solo");
}

#[tokio::test]
async fn test_ensure_queue_idempotent() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    let queue = "idempotent.queue";
    service.ensure_queue(queue);
    service.ensure_queue(queue);
    service.ensure_queue(queue);

    // Should still work normally
    let worker_id = WorkerId::new();
    let rx = service.register_worker(queue, PartitionId(0), worker_id);

    let envelope = make_envelope("t1", queue);
    assert!(service.offer_task(queue, PartitionId(0), envelope).is_ok());

    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "t1");
}

#[test]
fn test_config_accessor() {
    let config = MatchingConfig {
        num_partitions: 8,
        branching_factor: 4,
        max_buffer_per_partition: 500,
        task_reader_batch_size: 25,
        task_reader_poll_busy_ms: 5,
        task_reader_poll_idle_ms: 100,
    };
    let service = MatchingService::new(config.clone());
    assert_eq!(service.config().num_partitions, 8);
    assert_eq!(service.config().branching_factor, 4);
}
