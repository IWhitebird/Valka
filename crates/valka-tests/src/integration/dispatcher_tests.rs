//! Dispatcher behaviour against the engine.

use valka_core::TaskStatus;
use valka_proto::{Heartbeat, LogBatch, LogEntry, SignalAck, TaskResult};

use super::helpers::*;

fn result(
    task_id: &str,
    run_id: &str,
    success: bool,
    retryable: bool,
    output: &str,
    err: &str,
) -> TaskResult {
    TaskResult {
        task_id: task_id.into(),
        task_run_id: run_id.into(),
        success,
        retryable,
        output: output.into(),
        error_message: err.into(),
    }
}

#[tokio::test(start_paused = true)]
async fn test_handle_task_result_success_complex_json() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, &wid.0);
    node.dispatcher
        .handle_task_result(
            &wid,
            result(
                &t.id,
                &run,
                true,
                false,
                r#"{"a":[1,2,{"b":null}],"c":"d"}"#,
                "",
            ),
        )
        .await;
    let v = node.engine.get_task(&t.id).unwrap();
    assert_eq!(
        v.output,
        Some(serde_json::json!({"a":[1,2,{"b":null}],"c":"d"}))
    );
}

#[tokio::test(start_paused = true)]
async fn test_handle_task_result_success_empty_output() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, &wid.0);
    node.dispatcher
        .handle_task_result(&wid, result(&t.id, &run, true, false, "", ""))
        .await;
    let v = node.engine.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Completed);
    assert!(v.output.is_none());
}

#[tokio::test(start_paused = true)]
async fn test_handle_task_result_failure_non_retryable() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, &wid.0);
    node.dispatcher
        .handle_task_result(&wid, result(&t.id, &run, false, false, "", "bad"))
        .await;
    let v = node.engine.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Failed);
    assert_eq!(v.error_message.as_deref(), Some("bad"));
}

#[tokio::test(start_paused = true)]
async fn test_handle_task_result_retryable_cleans_worker_state() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, &wid.0);
    node.dispatcher
        .workers()
        .get_mut(wid.as_ref())
        .unwrap()
        .assign_task(t.id.clone());
    assert_eq!(
        node.dispatcher
            .workers()
            .get(wid.as_ref())
            .unwrap()
            .available_slots(),
        0
    );
    node.dispatcher
        .handle_task_result(&wid, result(&t.id, &run, false, true, "", "retry me"))
        .await;
    assert_eq!(
        node.dispatcher
            .workers()
            .get(wid.as_ref())
            .unwrap()
            .available_slots(),
        1
    );
    assert_eq!(
        node.engine.get_task(&t.id).unwrap().status,
        TaskStatus::Retry
    );
}

#[tokio::test(start_paused = true)]
async fn test_stale_result_for_wrong_run_is_ignored() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    let _run = node.start(&t.id, &wid.0);
    node.dispatcher
        .handle_task_result(&wid, result(&t.id, "not-the-run", true, false, "", ""))
        .await;
    assert_eq!(
        node.engine.get_task(&t.id).unwrap().status,
        TaskStatus::Running
    );
}

#[tokio::test(start_paused = true)]
async fn test_handle_heartbeat_extends_lease() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let t = node
        .create_with(valka_engine::CreateTask {
            timeout_seconds: 5,
            ..task_req("q", "t")
        })
        .await;
    node.start(&t.id, &wid.0);
    let before = node.engine.runs_for_task(&t.id).unwrap()[0].lease_expires_at;
    tokio::time::advance(std::time::Duration::from_secs(20)).await;
    node.dispatcher
        .handle_heartbeat(
            &wid,
            Heartbeat {
                active_task_ids: vec![t.id.clone()],
                timestamp_ms: 0,
            },
        )
        .await;
    let after = node.engine.runs_for_task(&t.id).unwrap()[0].lease_expires_at;
    assert!(after > before);
    // Still alive well past the original lease.
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(
        node.engine.get_task(&t.id).unwrap().status,
        TaskStatus::Running
    );
}

#[tokio::test(start_paused = true)]
async fn test_handle_log_batch_persists_chunks() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    node.dispatcher
        .handle_log_batch(
            &wid,
            LogBatch {
                entries: vec![
                    LogEntry {
                        task_run_id: "r1".into(),
                        timestamp_ms: 1,
                        level: 2,
                        message: "hello".into(),
                        metadata: String::new(),
                    },
                    LogEntry {
                        task_run_id: "r1".into(),
                        timestamp_ms: 2,
                        level: 4,
                        message: "oops".into(),
                        metadata: r#"{"k":1}"#.into(),
                    },
                ],
            },
        )
        .await;
    settle().await;
    let lines = node.logs.read("r1", 100).await;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].level, "INFO");
    assert_eq!(lines[1].level, "ERROR");
    assert_eq!(lines[1].metadata, Some(serde_json::json!({"k": 1})));
}

#[tokio::test(start_paused = true)]
async fn test_signal_ack_marks_acknowledged() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let s = node.engine.send_signal(&t.id, "x", None).await.unwrap();
    node.engine.signal_delivered(&s.id);
    settle().await;
    node.dispatcher
        .handle_signal_ack(&SignalAck {
            signal_id: s.id.clone(),
        })
        .await;
    settle().await;
    assert_eq!(
        node.engine.list_signals(&t.id, None)[0].status,
        "ACKNOWLEDGED"
    );
}

#[tokio::test(start_paused = true)]
async fn test_deregister_resets_delivered_signals() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    node.start(&t.id, &wid.0);
    node.dispatcher
        .workers()
        .get_mut(wid.as_ref())
        .unwrap()
        .assign_task(t.id.clone());
    let s = node.engine.send_signal(&t.id, "x", None).await.unwrap();
    node.engine.signal_delivered(&s.id);
    settle().await;
    assert_eq!(node.engine.list_signals(&t.id, None)[0].status, "DELIVERED");
    node.dispatcher.deregister_worker(&wid).await;
    settle().await;
    assert_eq!(node.engine.list_signals(&t.id, None)[0].status, "PENDING");
    assert_eq!(node.dispatcher.workers().len(), 0);
}

#[tokio::test(start_paused = true)]
async fn test_pending_signals_delivered_on_dispatch() {
    let node = TestNode::new().await;
    let (wid, mut rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    node.engine.send_signal(&t.id, "early", None).await.unwrap();
    // Drive the match loop so the dispatcher delivers the assignment + queued signal.
    let d = node.dispatcher.clone();
    let w = wid.clone();
    let h = tokio::spawn(async move { d.run_worker_match_loop(w, vec!["q".into()]).await });
    node.engine.unoffer(&t.id);
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        first.response,
        Some(valka_proto::worker_response::Response::TaskAssignment(_))
    ));
    let second = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(second.response, Some(valka_proto::worker_response::Response::TaskSignal(s)) if s.signal_name == "early")
    );
    settle().await;
    assert_eq!(node.engine.list_signals(&t.id, None)[0].status, "DELIVERED");
    h.abort();
}

#[tokio::test(start_paused = true)]
async fn test_retry_assignment_carries_checkpoints() {
    let node = TestNode::new().await;
    let (wid, mut rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, &wid.0);
    node.engine
        .checkpoint(&t.id, &run, "fetch", serde_json::json!({"rows": 3}))
        .await
        .unwrap();
    node.engine
        .fail_run(&t.id, &run, "boom", true)
        .await
        .unwrap();
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    settle().await;
    assert_eq!(
        node.engine.get_task(&t.id).unwrap().status,
        TaskStatus::Pending
    );

    let d = node.dispatcher.clone();
    let w = wid.clone();
    let h = tokio::spawn(async move { d.run_worker_match_loop(w, vec!["q".into()]).await });
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let Some(valka_proto::worker_response::Response::TaskAssignment(a)) = msg.response else {
        panic!("expected an assignment, got {:?}", msg.response);
    };
    assert_eq!(a.attempt_number, 2);
    assert_eq!(a.checkpoints.len(), 1);
    assert_eq!(a.checkpoints[0].step, "fetch");
    assert_eq!(a.checkpoints[0].attempt_number, 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&a.checkpoints[0].output).unwrap(),
        serde_json::json!({"rows": 3})
    );
    h.abort();
}

async fn next_ack(
    rx: &mut tokio::sync::mpsc::Receiver<valka_proto::WorkerResponse>,
) -> valka_proto::ResultAck {
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("an answer")
            .expect("stream open");
        if let Some(valka_proto::worker_response::Response::ResultAck(ack)) = msg.response {
            return ack;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn test_every_result_is_answered_with_a_result_ack() {
    use valka_proto::ResultStatus;
    let node = TestNode::new().await;
    let (wid, mut rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, &wid.0);

    let ok = result(&t.id, &run, true, false, r#"{"n":1}"#, "");
    node.dispatcher.handle_task_result(&wid, ok.clone()).await;
    let ack = next_ack(&mut rx).await;
    assert_eq!(ack.status, ResultStatus::Applied as i32);
    assert_eq!(
        (ack.task_id.as_str(), ack.task_run_id.as_str()),
        (t.id.as_str(), run.as_str())
    );

    node.dispatcher.handle_task_result(&wid, ok).await;
    assert_eq!(
        next_ack(&mut rx).await.status,
        ResultStatus::Applied as i32,
        "a resend is idempotent"
    );

    let c = node.create("q", "t").await;
    let crun = node.start(&c.id, &wid.0);
    node.engine.cancel_task(&c.id, "user").await.unwrap();
    node.dispatcher
        .handle_task_result(&wid, result(&c.id, &crun, true, false, "", ""))
        .await;
    assert_eq!(next_ack(&mut rx).await.status, ResultStatus::Stale as i32);
    assert_eq!(
        node.engine.get_task(&c.id).unwrap().status,
        TaskStatus::Cancelled
    );
}

#[tokio::test(start_paused = true)]
async fn test_reconnecting_worker_keeps_current_runs_and_cancels_stale_ones() {
    use valka_proto::{RunningTask, worker_response};
    let node = TestNode::new().await;
    node.engine
        .set_sink(std::sync::Arc::new(valka_engine::NoopSink));
    let live = node.create("q", "t").await;
    let live_run = node.start(&live.id, "w-old");
    let done = node.create("q", "t").await;
    node.complete(&done.id).await;
    let done_run = node.engine.runs_for_task(&done.id).unwrap()[0].id.clone();
    let lost = node.create("q", "t").await;
    let lost_run = valka_core::new_id_in_shard(node.engine.shard_of(&lost.id).unwrap()).to_string();

    let (wid, mut rx) = node.register_worker(&["q"], 4).await;
    let handle_tx = node
        .dispatcher
        .workers()
        .get(wid.as_ref())
        .unwrap()
        .response_tx
        .clone();
    node.dispatcher
        .reattach_running(
            &wid,
            &[
                RunningTask {
                    task_id: live.id.clone(),
                    task_run_id: live_run.clone(),
                    attempt_number: 1,
                },
                RunningTask {
                    task_id: done.id.clone(),
                    task_run_id: done_run,
                    attempt_number: 1,
                },
                RunningTask {
                    task_id: lost.id.clone(),
                    task_run_id: lost_run.clone(),
                    attempt_number: 1,
                },
            ],
            &handle_tx,
        )
        .await;
    // Only the finished run is cancelled on the worker.
    let msg = rx.recv().await.unwrap();
    match msg.response {
        Some(worker_response::Response::TaskCancellation(c)) => assert_eq!(c.task_id, done.id),
        other => panic!("expected a cancellation, got {other:?}"),
    }
    assert!(rx.try_recv().is_err());
    // The live run now belongs to the new worker; the lost dispatch was adopted.
    settle().await;
    let loc = node.engine.run_location(&live.id).unwrap();
    assert_eq!(loc.worker_id, wid.0);
    assert_eq!(
        node.engine.get_task(&lost.id).unwrap().status,
        TaskStatus::Running
    );
    let active = node
        .dispatcher
        .workers()
        .get(wid.as_ref())
        .unwrap()
        .active_tasks
        .clone();
    assert!(active.contains(&live.id) && active.contains(&lost.id) && !active.contains(&done.id));
}

/// Fails the first `fail` reattaches (the owner unreachable while its shard moves), then
/// answers like the local engine. With `fail_dispatches`, dispatches fail the same way.
struct FlakyReattach {
    inner: valka_dispatcher::LocalRouter,
    fail: std::sync::atomic::AtomicU32,
    fail_dispatches: std::sync::atomic::AtomicU32,
}

fn take_failure(n: &std::sync::atomic::AtomicU32) -> bool {
    use std::sync::atomic::Ordering;
    let mut cur = n.load(Ordering::SeqCst);
    while cur > 0 {
        match n.compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(seen) => cur = seen,
        }
    }
    false
}

#[async_trait::async_trait]
impl valka_dispatcher::TaskRouter for FlakyReattach {
    async fn dispatch(
        &self,
        task_id: &str,
        worker_id: &str,
    ) -> Result<valka_engine::DispatchInfo, valka_core::ServerError> {
        if take_failure(&self.fail_dispatches) {
            return Err(valka_core::ServerError::Unavailable(
                "owner briefly not serving".into(),
            ));
        }
        self.inner.dispatch(task_id, worker_id).await
    }
    async fn return_run(&self, task_id: &str, run_id: &str) {
        self.inner.return_run(task_id, run_id).await
    }
    async fn unoffer(&self, task_id: &str) {
        self.inner.unoffer(task_id).await
    }
    async fn report_result(
        &self,
        task_id: &str,
        run_id: &str,
        result: valka_engine::RunResult,
    ) -> valka_engine::ResultOutcome {
        self.inner.report_result(task_id, run_id, result).await
    }
    async fn heartbeat(&self, task_ids: &[String]) {
        self.inner.heartbeat(task_ids).await
    }
    async fn checkpoint(
        &self,
        task_id: &str,
        run_id: &str,
        step: &str,
        output: serde_json::Value,
    ) -> Result<valka_engine::CheckpointView, valka_core::ServerError> {
        self.inner.checkpoint(task_id, run_id, step, output).await
    }
    async fn signal_delivered(&self, signal_id: &str) {
        self.inner.signal_delivered(signal_id).await
    }
    async fn signal_acked(&self, signal_id: &str) {
        self.inner.signal_acked(signal_id).await
    }
    async fn reset_signals(&self, task_id: &str) {
        self.inner.reset_signals(task_id).await
    }
    async fn reattach(
        &self,
        task_id: &str,
        run_id: &str,
        attempt: i32,
        worker_id: &str,
    ) -> Result<valka_engine::ReattachOutcome, valka_core::ServerError> {
        if take_failure(&self.fail) {
            return Err(valka_core::ServerError::Unavailable(
                "shard is moving".into(),
            ));
        }
        self.inner
            .reattach(task_id, run_id, attempt, worker_id)
            .await
    }
}

#[tokio::test(start_paused = true)]
async fn test_reattach_is_retried_while_the_owner_is_unreachable() {
    use valka_proto::RunningTask;
    let node = TestNode::new().await;
    node.engine
        .set_sink(std::sync::Arc::new(valka_engine::NoopSink));
    let live = node.create("q", "t").await;
    let live_run = node.start(&live.id, "w-old");
    // A dispatch record lost in a crash: only a successful reattach adopts it.
    let lost = node.create("q", "t").await;
    let lost_run = valka_core::new_id_in_shard(node.engine.shard_of(&lost.id).unwrap()).to_string();
    node.dispatcher
        .set_router(std::sync::Arc::new(FlakyReattach {
            inner: valka_dispatcher::LocalRouter(node.engine.clone()),
            fail: std::sync::atomic::AtomicU32::new(4),
            fail_dispatches: std::sync::atomic::AtomicU32::new(0),
        }));

    let (wid, _rx) = node.register_worker(&["q"], 4).await;
    let tx = node
        .dispatcher
        .workers()
        .get(wid.as_ref())
        .unwrap()
        .response_tx
        .clone();
    node.dispatcher
        .reattach_running(
            &wid,
            &[
                RunningTask {
                    task_id: live.id.clone(),
                    task_run_id: live_run.clone(),
                    attempt_number: 1,
                },
                RunningTask {
                    task_id: lost.id.clone(),
                    task_run_id: lost_run.clone(),
                    attempt_number: 1,
                },
            ],
            &tx,
        )
        .await;
    // Both failed: nothing changed at the owner yet.
    assert_eq!(
        node.engine.run_location(&live.id).unwrap().worker_id,
        "w-old"
    );
    assert_eq!(
        node.engine.get_task(&lost.id).unwrap().status,
        TaskStatus::Pending
    );

    // The background retries get through once the owner answers again.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    settle().await;
    assert_eq!(node.engine.run_location(&live.id).unwrap().worker_id, wid.0);
    assert_eq!(
        node.engine.get_task(&lost.id).unwrap().status,
        TaskStatus::Running
    );
    assert_eq!(node.engine.run_location(&lost.id).unwrap().worker_id, wid.0);
}

#[tokio::test(start_paused = true)]
async fn test_a_failed_dispatch_hands_the_task_back_at_once() {
    // Found by the chaos rig: a dispatch refused while the owner was briefly not serving
    // left the task offered (PENDING, out of the pending index) instead of runnable.
    let node = TestNode::new().await;
    node.dispatcher
        .set_router(std::sync::Arc::new(FlakyReattach {
            inner: valka_dispatcher::LocalRouter(node.engine.clone()),
            fail: std::sync::atomic::AtomicU32::new(0),
            fail_dispatches: std::sync::atomic::AtomicU32::new(1),
        }));
    let (wid, mut rx) = node.register_worker(&["q"], 1).await;
    let (d, w) = (node.dispatcher.clone(), wid.clone());
    tokio::spawn(async move { d.run_worker_match_loop(w, vec!["q".to_string()]).await });
    settle().await;
    let task = node.create("q", "t").await;
    // The first dispatch fails; the task goes back and the next offer reaches the worker,
    // long before the offer's 30 s lapse would have rescued it.
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("assignment after the failed dispatch")
        .unwrap();
    match msg.response {
        Some(valka_proto::worker_response::Response::TaskAssignment(a)) => {
            assert_eq!(a.task_id, task.id)
        }
        other => panic!("expected an assignment, got {other:?}"),
    }
    assert_eq!(
        node.engine.get_task(&task.id).unwrap().status,
        TaskStatus::Running
    );
}

fn assignments(
    rx: &mut tokio::sync::mpsc::Receiver<valka_proto::WorkerResponse>,
) -> Vec<valka_proto::TaskAssignment> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let Some(valka_proto::worker_response::Response::TaskAssignment(a)) = msg.response {
            out.push(a);
        }
    }
    out
}

#[tokio::test(start_paused = true)]
async fn test_a_worker_takes_one_task_per_free_slot_and_no_more() {
    // Found by the benchmark: a worker took one task per round, the feeder offered one
    // task per waiting worker whatever its free slots, and tasks the feeder had taken but
    // not offered sat out the 30 s offer lapse.
    let node = TestNode::new().await;
    for _ in 0..10 {
        node.create("q", "t").await;
    }
    let (wid, mut rx) = node.register_worker(&["q"], 4).await;
    let (d, w) = (node.dispatcher.clone(), wid.clone());
    tokio::spawn(async move { d.run_worker_match_loop(w, vec!["q".to_string()]).await });
    settle().await;
    let first = assignments(&mut rx);
    assert_eq!(first.len(), 4, "one task per free slot");
    assert_eq!(
        node.engine.pending_count("q"),
        6,
        "the rest stay runnable; none is stranded as offered"
    );

    // A result frees a slot and the next task follows at once.
    let a = &first[0];
    node.dispatcher
        .handle_task_result(
            &wid,
            result(&a.task_id, &a.task_run_id, true, false, "", ""),
        )
        .await;
    settle().await;
    assert_eq!(assignments(&mut rx).len(), 1);
    assert_eq!(node.engine.pending_count("q"), 5);
}
