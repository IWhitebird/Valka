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
