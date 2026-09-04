use futures::FutureExt;
use std::future::Future;
use std::pin::Pin;
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
        feeder_interval_ms: 5,
        feeder_batch_size: 25,
    };
    let service = MatchingService::new(config.clone());
    assert_eq!(service.config().num_partitions, 8);
    assert_eq!(service.config().branching_factor, 4);
}

// ─── select_all Recovery (validates the fixed bug) ──────────────────

#[tokio::test]
async fn test_multi_partition_buffer_only_one_consumed() {
    let config = MatchingConfig::default(); // n=4, bf=3
    let service = MatchingService::new(config);
    let queue = "test.queue";
    service.ensure_queue(queue);

    // Buffer 1 task on each of P0, P1, P2
    service.buffer_task(queue, PartitionId(0), make_envelope("t0", queue));
    service.buffer_task(queue, PartitionId(1), make_envelope("t1", queue));
    service.buffer_task(queue, PartitionId(2), make_envelope("t2", queue));

    // Register worker on all 3 — each rx resolves immediately via register_worker
    let w = WorkerId::new();
    let rx0 = service.register_worker(queue, PartitionId(0), w.clone());
    let rx1 = service.register_worker(queue, PartitionId(1), w.clone());
    let rx2 = service.register_worker(queue, PartitionId(2), w.clone());

    // Simulate select_all: only consume 1, buffer back the others
    type PidFut = Pin<
        Box<
            dyn Future<
                Output = (
                    PartitionId,
                    Result<TaskEnvelope, tokio::sync::oneshot::error::RecvError>,
                ),
            >,
        >,
    >;
    let futs: Vec<PidFut> = vec![
        Box::pin(async move { (PartitionId(0), rx0.await) }),
        Box::pin(async move { (PartitionId(1), rx1.await) }),
        Box::pin(async move { (PartitionId(2), rx2.await) }),
    ];

    let (first, _idx, remaining) = futures::future::select_all(futs).await;
    let consumed_id = first.1.unwrap().task_id;

    // Buffer back remaining resolved receivers (the bug fix)
    let mut buffered_count = 0;
    for fut in remaining {
        if let Some((pid, Ok(envelope))) = fut.now_or_never() {
            service.buffer_task(queue, pid, envelope);
            buffered_count += 1;
        }
    }
    assert_eq!(buffered_count, 2, "Should buffer back 2 unprocessed tasks");

    // Register fresh workers on each partition
    let mut received = Vec::new();
    for pid in 0..3 {
        let rx = service.register_worker(queue, PartitionId(pid), WorkerId::new());
        if let Some(Ok(env)) = rx.now_or_never() {
            received.push(env.task_id);
        }
    }

    // 2 of the 3 partitions should have tasks
    assert_eq!(received.len(), 2, "Should receive 2 buffered-back tasks");

    // All 3 tasks should be accounted for
    received.push(consumed_id);
    received.sort();
    assert_eq!(received, vec!["t0", "t1", "t2"]);
}

#[tokio::test]
async fn test_multi_partition_no_buffered_tasks_worker_waits() {
    let config = MatchingConfig::default(); // n=4
    let service = MatchingService::new(config);
    let queue = "test.queue";
    service.ensure_queue(queue);

    // Register worker on P0 and P2 with no buffered tasks
    let w = WorkerId::new();
    let rx0 = service.register_worker(queue, PartitionId(0), w.clone());
    let rx2 = service.register_worker(queue, PartitionId(2), w.clone());

    // Both receivers should be pending (no tasks buffered)
    assert!(rx0.now_or_never().is_none(), "P0 should be pending");

    // Offer a task on P2 — should resolve the waiting worker on P2
    let envelope = make_envelope("task-p2", queue);
    let result = service.offer_task(queue, PartitionId(2), envelope);
    assert!(result.is_ok(), "Should match worker waiting on P2");

    let received = rx2.await.unwrap();
    assert_eq!(received.task_id, "task-p2");
}

#[tokio::test]
async fn test_multi_partition_all_buffered_separate_workers() {
    let config = MatchingConfig::default(); // n=4
    let service = MatchingService::new(config);
    let queue = "test.queue";
    service.ensure_queue(queue);

    // Buffer 1 task on each of 4 partitions
    for pid in 0..4 {
        service.buffer_task(
            queue,
            PartitionId(pid),
            make_envelope(&format!("t{pid}"), queue),
        );
    }

    // Register 4 separate workers (one per partition)
    let mut received = Vec::new();
    for pid in 0..4 {
        let rx = service.register_worker(queue, PartitionId(pid), WorkerId::new());
        let env = rx.await.expect("Each worker should get a task");
        received.push(env.task_id);
    }

    received.sort();
    assert_eq!(received, vec!["t0", "t1", "t2", "t3"]);
}

#[tokio::test]
async fn test_register_worker_stale_slot_plus_pending_task() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);
    let queue = "test.queue";
    service.ensure_queue(queue);

    // Register worker A, then drop its receiver (simulating disconnect)
    let rx_a = service.register_worker(queue, PartitionId(0), WorkerId::new());
    drop(rx_a);

    // Buffer a task
    service.buffer_task(queue, PartitionId(0), make_envelope("t1", queue));

    // Register worker B — should get the buffered task (stale slot A is gone)
    let rx_b = service.register_worker(queue, PartitionId(0), WorkerId::new());
    let received = rx_b
        .await
        .expect("Worker B should receive the buffered task");
    assert_eq!(received.task_id, "t1");
}

// ─── Partition Tree Edge Cases ──────────────────────────────────────

#[tokio::test]
async fn test_tree_forwarding_worker_at_leaf_task_at_root() {
    // bf=2, n=8: P7 is a leaf (child of P3), P0 is root
    // Forwarding is upward only — worker on P7 can't receive task offered at P0
    let mut config = MatchingConfig::default();
    config.num_partitions = 8;
    config.branching_factor = 2;
    let service = MatchingService::new(config);
    let queue = "tree.queue";
    service.ensure_queue(queue);

    let _rx = service.register_worker(queue, PartitionId(7), WorkerId::new());

    let envelope = make_envelope("root-task", queue);
    let result = service.offer_task(queue, PartitionId(0), envelope);
    assert!(
        result.is_err(),
        "Task on root should NOT match worker on leaf (forwarding is upward only)"
    );
    assert_eq!(result.unwrap_err().task_id, "root-task");
}

#[tokio::test]
async fn test_tree_forwarding_worker_at_sibling_no_match() {
    // Default config: n=4, bf=3. P1, P2, P3 are siblings under P0.
    // Worker on P1, offer on P2. No worker on P0. Should not match.
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);
    let queue = "sibling.queue";
    service.ensure_queue(queue);

    let _rx = service.register_worker(queue, PartitionId(1), WorkerId::new());

    let envelope = make_envelope("sib-task", queue);
    let result = service.offer_task(queue, PartitionId(2), envelope);
    assert!(
        result.is_err(),
        "Task on P2 should not match worker on P1 (siblings, no common ancestor worker)"
    );
}

#[tokio::test]
async fn test_tree_forwarding_grandchild_to_root() {
    // bf=2, n=8: P3's parent = P1, P1's parent = P0
    let mut config = MatchingConfig::default();
    config.num_partitions = 8;
    config.branching_factor = 2;
    let service = MatchingService::new(config);
    let queue = "grandchild.queue";
    service.ensure_queue(queue);

    let worker_id = WorkerId::new();
    let rx = service.register_worker(queue, PartitionId(0), worker_id);

    let envelope = make_envelope("gc-task", queue);
    let result = service.offer_task(queue, PartitionId(3), envelope);
    assert!(result.is_ok(), "Should match via P3 → P1 → P0");

    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "gc-task");
}

#[tokio::test]
async fn test_large_tree_16_partitions() {
    // bf=4, n=16: P15 → parent P3 → parent P0 (root)
    let mut config = MatchingConfig::default();
    config.num_partitions = 16;
    config.branching_factor = 4;
    let service = MatchingService::new(config);
    let queue = "large.queue";
    service.ensure_queue(queue);

    let rx = service.register_worker(queue, PartitionId(0), WorkerId::new());

    let envelope = make_envelope("deep-leaf", queue);
    let result = service.offer_task(queue, PartitionId(15), envelope);
    assert!(result.is_ok(), "Should match via P15 → P3 → P0");

    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "deep-leaf");
}

#[tokio::test]
async fn test_flat_tree_branching_equals_partitions() {
    // bf=4, n=4: All children of root. One hop forwarding.
    let mut config = MatchingConfig::default();
    config.num_partitions = 4;
    config.branching_factor = 4;
    let service = MatchingService::new(config);
    let queue = "flat.queue";
    service.ensure_queue(queue);

    let rx = service.register_worker(queue, PartitionId(0), WorkerId::new());

    let envelope = make_envelope("flat-task", queue);
    let result = service.offer_task(queue, PartitionId(3), envelope);
    assert!(result.is_ok(), "Should match via P3 → P0 (one hop)");

    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "flat-task");
}

// ─── Buffer & Registration ──────────────────────────────────────────

#[tokio::test]
async fn test_buffer_full_returns_false_task_not_lost() {
    let mut config = MatchingConfig::default();
    config.max_buffer_per_partition = 1;
    let service = MatchingService::new(config);
    let queue = "buffer.queue";
    service.ensure_queue(queue);

    assert!(
        service.buffer_task(queue, PartitionId(0), make_envelope("t1", queue)),
        "First buffer should succeed"
    );
    assert!(
        !service.buffer_task(queue, PartitionId(0), make_envelope("t2", queue)),
        "Second buffer should fail (max_buffer=1)"
    );

    // Register worker — gets t1 only
    let rx = service.register_worker(queue, PartitionId(0), WorkerId::new());
    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "t1");
}

#[tokio::test]
async fn test_register_worker_buffer_fifo_order() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);
    let queue = "fifo.queue";
    service.ensure_queue(queue);

    // Buffer t1, t2, t3 on P0
    service.buffer_task(queue, PartitionId(0), make_envelope("t1", queue));
    service.buffer_task(queue, PartitionId(0), make_envelope("t2", queue));
    service.buffer_task(queue, PartitionId(0), make_envelope("t3", queue));

    // Register one worker — gets t1 (FIFO from VecDeque)
    let rx = service.register_worker(queue, PartitionId(0), WorkerId::new());
    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "t1", "Should dequeue in FIFO order");
}

#[tokio::test]
async fn test_multiple_queues_buffer_isolation() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    service.ensure_queue("queue-a");
    service.ensure_queue("queue-b");

    // Buffer on queue-a P0 and queue-b P0
    service.buffer_task(
        "queue-a",
        PartitionId(0),
        make_envelope("task-a", "queue-a"),
    );
    service.buffer_task(
        "queue-b",
        PartitionId(0),
        make_envelope("task-b", "queue-b"),
    );

    // Register worker on queue-a only
    let rx = service.register_worker("queue-a", PartitionId(0), WorkerId::new());
    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "task-a", "Should only get queue-a task");

    // queue-b task should still be buffered
    let rx_b = service.register_worker("queue-b", PartitionId(0), WorkerId::new());
    let received_b = rx_b.await.unwrap();
    assert_eq!(received_b.task_id, "task-b");
}

#[tokio::test]
async fn test_concurrent_offer_task_same_partition() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);
    let queue = "concurrent.queue";
    service.ensure_queue(queue);

    // Register 10 workers on P0
    let mut receivers = Vec::new();
    for _ in 0..10 {
        let rx = service.register_worker(queue, PartitionId(0), WorkerId::new());
        receivers.push(rx);
    }

    // Spawn 10 concurrent offer_task calls
    let mut handles = Vec::new();
    for i in 0..10 {
        let svc = service.clone();
        let q = queue.to_string();
        handles.push(tokio::spawn(async move {
            let envelope = TaskEnvelope {
                task_id: format!("t{i}"),
                task_run_id: String::new(),
                queue_name: q.clone(),
                task_name: "test_task".to_string(),
                input: None,
                attempt_number: 1,
                timeout_seconds: 300,
                metadata: "{}".to_string(),
                priority: 0,
            };
            svc.offer_task(&q, PartitionId(0), envelope)
        }));
    }

    // All should complete without panic
    let mut matched = 0;
    for handle in handles {
        if handle.await.unwrap().is_ok() {
            matched += 1;
        }
    }
    assert_eq!(
        matched, 10,
        "All 10 tasks should match with waiting workers"
    );

    // All 10 receivers should have received a task
    for rx in receivers {
        assert!(rx.await.is_ok(), "Each worker should get a task");
    }
}

// ─── Deregister ─────────────────────────────────────────────────────

#[test]
fn test_deregister_nonexistent_worker_no_panic() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);
    service.ensure_queue("test.queue");

    // Should not panic
    service.deregister_worker(&WorkerId::new());
}

#[tokio::test]
async fn test_deregister_then_reregister_same_id() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);
    let queue = "test.queue";
    service.ensure_queue(queue);

    let worker_id = WorkerId::new();

    // Register, deregister, then re-register with same ID
    let _rx1 = service.register_worker(queue, PartitionId(0), worker_id.clone());
    service.deregister_worker(&worker_id);
    let rx2 = service.register_worker(queue, PartitionId(0), worker_id);

    // Offer task — should match the re-registered worker
    let envelope = make_envelope("t1", queue);
    let result = service.offer_task(queue, PartitionId(0), envelope);
    assert!(result.is_ok(), "Should match re-registered worker");

    let received = rx2.await.unwrap();
    assert_eq!(received.task_id, "t1");
}

#[tokio::test]
async fn test_buffer_task_on_new_queue_auto_creates() {
    let config = MatchingConfig::default();
    let service = MatchingService::new(config);

    // Don't call ensure_queue — buffer_task calls it internally
    let result = service.buffer_task(
        "brand-new-queue",
        PartitionId(0),
        make_envelope("t1", "brand-new-queue"),
    );
    assert!(result, "buffer_task should auto-create queue and succeed");

    // Verify task is buffered by registering a worker
    let rx = service.register_worker("brand-new-queue", PartitionId(0), WorkerId::new());
    let received = rx.await.unwrap();
    assert_eq!(received.task_id, "t1");
}
