//! Task lifecycle through the engine + dispatcher, with paused time for timers.

use std::time::Duration;

use valka_core::TaskStatus;
use valka_engine::CreateTask;
use valka_proto::{TaskResult, worker_response};

use super::helpers::*;

#[tokio::test(start_paused = true)]
async fn test_task_create_dispatch_complete() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 4).await;
    let t = node.create("q", "t").await;
    let d = node.engine.dispatch(&t.id, &wid.0).unwrap();
    assert_eq!(
        node.engine.get_task(&t.id).unwrap().status,
        TaskStatus::Running
    );
    node.dispatcher
        .handle_task_result(
            &wid,
            TaskResult {
                task_id: t.id.clone(),
                task_run_id: d.run_id.clone(),
                success: true,
                retryable: false,
                output: r#"{"result": 42}"#.into(),
                error_message: String::new(),
            },
        )
        .await;
    let v = node.engine.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Completed);
    assert_eq!(v.output, Some(serde_json::json!({"result": 42})));
    assert_eq!(v.attempt_count, 1);
    let runs = node.engine.runs_for_task(&t.id).unwrap();
    assert_eq!(runs[0].status, "COMPLETED");
    assert_eq!(runs[0].output, Some(serde_json::json!({"result": 42})));
}

#[tokio::test(start_paused = true)]
async fn test_task_fail_retry_succeed() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 4).await;
    let t = node.create("q", "t").await;
    let d1 = node.engine.dispatch(&t.id, &wid.0).unwrap();
    node.dispatcher
        .handle_task_result(
            &wid,
            TaskResult {
                task_id: t.id.clone(),
                task_run_id: d1.run_id,
                success: false,
                retryable: true,
                output: String::new(),
                error_message: "transient".into(),
            },
        )
        .await;
    assert_eq!(
        node.engine.get_task(&t.id).unwrap().status,
        TaskStatus::Retry
    );
    tokio::time::advance(Duration::from_secs(3)).await;
    settle().await;
    let v = node.engine.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Pending, "promoted after backoff");
    let d2 = node.engine.dispatch(&t.id, &wid.0).unwrap();
    assert_eq!(d2.attempt, 2);
    node.engine
        .complete_run(&t.id, &d2.run_id, None)
        .await
        .unwrap();
    let v = node.engine.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Completed);
    assert_eq!(v.attempt_count, 2);
    assert_eq!(node.engine.runs_for_task(&t.id).unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn test_task_exhaust_retries_to_dlq() {
    let node = TestNode::new().await;
    let t = node
        .create_with(CreateTask {
            max_retries: 2,
            ..task_req("q", "t")
        })
        .await;
    let v = node.dead_letter(&t.id).await;
    assert_eq!(v.status, TaskStatus::DeadLetter);
    assert_eq!(v.attempt_count, 2);
    assert_eq!(node.engine.list_dead_letters(None, 10, 0).len(), 1);
    assert_eq!(node.engine.runs_for_task(&t.id).unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn test_max_retries_zero_immediate_dlq() {
    let node = TestNode::new().await;
    let t = node
        .create_with(CreateTask {
            max_retries: 0,
            ..task_req("q", "t")
        })
        .await;
    let run = node.start(&t.id, "w");
    let r = node.engine.fail_run(&t.id, &run, "x", true).await.unwrap();
    assert_eq!(r.outcome, valka_wal::FailureOutcome::DeadLetter);
}

#[tokio::test(start_paused = true)]
async fn test_cancel_pending_task() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let (v, worker) = node.engine.cancel_task(&t.id, "user").await.unwrap();
    assert_eq!(v.status, TaskStatus::Cancelled);
    assert!(worker.is_none());
    assert!(node.engine.dispatch(&t.id, "w").is_err());
}

#[tokio::test(start_paused = true)]
async fn test_cancel_running_task_via_dispatcher() {
    let node = TestNode::new().await;
    let (wid, mut rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    node.engine.dispatch(&t.id, &wid.0).unwrap();
    node.dispatcher
        .workers()
        .get_mut(wid.as_ref())
        .unwrap()
        .assign_task(t.id.clone());
    assert!(node.dispatcher.cancel_task_on_worker(&t.id).await);
    let msg = rx.recv().await.unwrap();
    assert!(matches!(
        msg.response,
        Some(worker_response::Response::TaskCancellation(_))
    ));
    assert!(!node.dispatcher.cancel_task_on_worker("other").await);
}

#[tokio::test(start_paused = true)]
async fn test_scheduled_task_not_dispatched_early() {
    let node = TestNode::new().await;
    let at = node.engine.clock().now() + chrono::Duration::seconds(120);
    let t = node
        .create_with(CreateTask {
            scheduled_at: Some(at),
            ..task_req("q", "t")
        })
        .await;
    assert!(node.engine.dispatch(&t.id, "w").is_err());
    assert_eq!(node.engine.pending_count("q"), 0);
    tokio::time::advance(Duration::from_secs(121)).await;
    settle().await;
    assert!(node.engine.dispatch(&t.id, "w").is_ok());
}

#[tokio::test(start_paused = true)]
async fn test_idempotency_key_prevents_duplicate() {
    let node = TestNode::new().await;
    let req = CreateTask {
        idempotency_key: Some("k".into()),
        ..task_req("q", "t")
    };
    node.create_with(req.clone()).await;
    let err = node.engine.create_task(req).await.unwrap_err();
    assert!(matches!(
        err,
        valka_core::ServerError::IdempotencyConflict(_)
    ));
    assert_eq!(node.engine.list_tasks(None, None, 10, 0).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn test_priority_then_fifo_ordering() {
    let node = TestNode::new().await;
    let low = node
        .create_with(CreateTask {
            priority: 0,
            ..task_req("q", "t")
        })
        .await;
    tokio::time::advance(Duration::from_millis(5)).await;
    let high = node
        .create_with(CreateTask {
            priority: 10,
            ..task_req("q", "t")
        })
        .await;
    tokio::time::advance(Duration::from_millis(5)).await;
    let low2 = node
        .create_with(CreateTask {
            priority: 0,
            ..task_req("q", "t")
        })
        .await;
    settle().await;
    // Everything went into matching buffers (no worker) in offer order; check the
    // engine's own ordering by pulling them back out of the pending index.
    for id in [&low.id, &high.id, &low2.id] {
        node.engine.unoffer(id);
    }
    // Detach the sink so take_pending is the only consumer.
    node.engine
        .set_sink(std::sync::Arc::new(valka_engine::NoopSink));
    for id in [&low.id, &high.id, &low2.id] {
        node.engine.unoffer(id);
    }
    let order: Vec<String> = node
        .engine
        .take_pending("q", 10)
        .into_iter()
        .map(|t| t.task_id)
        .collect();
    assert_eq!(order, vec![high.id, low.id, low2.id]);
}

#[tokio::test(start_paused = true)]
async fn test_multiple_tasks_reach_workers_via_matching() {
    let node = TestNode::new().await;
    let (wid, mut rx) = node.register_worker(&["q"], 2).await;
    // Run the match loop like the stream handler does.
    let d = node.dispatcher.clone();
    let w = wid.clone();
    let loop_handle =
        tokio::spawn(async move { d.run_worker_match_loop(w, vec!["q".into()]).await });
    let a = node.create("q", "t").await;
    let b = node.create("q", "t").await;
    let mut got = Vec::new();
    for _ in 0..2 {
        let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let Some(worker_response::Response::TaskAssignment(ta)) = msg.response {
            got.push(ta.task_id);
        }
    }
    got.sort();
    let mut want = vec![a.id.clone(), b.id.clone()];
    want.sort();
    assert_eq!(got, want);
    assert_eq!(
        node.engine.get_task(&a.id).unwrap().status,
        TaskStatus::Running
    );
    assert_eq!(
        node.dispatcher
            .workers()
            .get(wid.as_ref())
            .unwrap()
            .available_slots(),
        0
    );
    loop_handle.abort();
}

#[tokio::test(start_paused = true)]
async fn test_lease_expiry_after_worker_death() {
    let node = TestNode::new().await;
    let t = node
        .create_with(CreateTask {
            timeout_seconds: 10,
            ..task_req("q", "t")
        })
        .await;
    node.start(&t.id, "w");
    tokio::time::advance(Duration::from_secs(41)).await; // 10 + 30 grace
    settle().await;
    let v = node.engine.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Retry);
    assert_eq!(
        node.engine.runs_for_task(&t.id).unwrap()[0]
            .error_message
            .as_deref(),
        Some("Lease expired")
    );
}

#[tokio::test(start_paused = true)]
async fn test_crash_recovery_keeps_running_tasks_and_pending_index() {
    let store = valka_wal::Store::memory();
    let (p, r, run);
    {
        let node = TestNode::on_store(store.clone(), "n").await;
        p = node.create("q", "t").await;
        r = node.create("q", "t").await;
        run = node.start(&r.id, "w");
        node.engine.sync().await.unwrap();
    }
    let node = TestNode::on_store(store, "n").await;
    node.engine
        .set_sink(std::sync::Arc::new(valka_engine::NoopSink));
    assert_eq!(
        node.engine.get_task(&r.id).unwrap().status,
        TaskStatus::Running
    );
    // The worker finishes after the restart: accepted.
    node.engine.complete_run(&r.id, &run, None).await.unwrap();
    // Pending task is still dispatchable.
    assert!(node.engine.dispatch(&p.id, "w2").is_ok());
}
