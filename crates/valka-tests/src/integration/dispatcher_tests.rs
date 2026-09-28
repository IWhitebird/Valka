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
