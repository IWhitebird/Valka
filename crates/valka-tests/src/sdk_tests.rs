use tokio::sync::mpsc;
use valka_proto::{TaskSignal, WorkerRequest, worker_request};
use valka_sdk::context::{SignalData, TaskContext};
use valka_sdk::retry::RetryPolicy;

#[test]
fn test_worker_builder_defaults() {
    let builder = valka_sdk::worker::ValkaWorkerBuilder::new();
    // We can't directly access private fields, but we can verify the builder exists
    // and has the correct Default impl
    let builder2 = valka_sdk::worker::ValkaWorkerBuilder::default();
    // Both should create without panic
    drop(builder);
    drop(builder2);
}

#[tokio::test]
async fn test_worker_builder_without_handler_fails() {
    let builder = valka_sdk::worker::ValkaWorkerBuilder::new();
    let result = builder
        .name("test")
        .queues(&["demo"])
        .concurrency(2)
        .build()
        .await;

    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("Building without handler should fail"),
    };
    assert!(
        format!("{err}").contains("handler"),
        "Error should mention handler, got: {err}"
    );
}

#[tokio::test]
async fn test_worker_builder_fluent_api() {
    let result = valka_sdk::ValkaWorker::builder()
        .name("my-worker")
        .server_addr("http://localhost:50051")
        .queues(&["queue-a", "queue-b"])
        .concurrency(4)
        .metadata("{\"version\": 1}")
        .handler(|_ctx| async { Ok(serde_json::json!({})) })
        .build()
        .await;

    assert!(result.is_ok(), "Builder with all fields should succeed");
}

#[test]
fn test_retry_policy_initial_delay() {
    let mut policy = RetryPolicy::new();
    let d = policy.next_delay();
    // Initial delay is 100ms + up to 10% jitter
    assert!(
        d.as_millis() >= 100 && d.as_millis() <= 120,
        "First delay should be 100-120ms, got {}ms",
        d.as_millis()
    );
}

#[test]
fn test_retry_policy_exponential_growth() {
    let mut policy = RetryPolicy::new();
    let d1 = policy.next_delay();
    let d2 = policy.next_delay();
    let d3 = policy.next_delay();
    let d4 = policy.next_delay();

    // Each should be roughly double the previous (within jitter)
    assert!(d2 > d1, "d2 ({d2:?}) should be > d1 ({d1:?})");
    assert!(d3 > d2, "d3 ({d3:?}) should be > d2 ({d2:?})");
    assert!(d4 > d3, "d4 ({d4:?}) should be > d3 ({d3:?})");
}

#[test]
fn test_retry_policy_capped_at_max() {
    let mut policy = RetryPolicy::new();

    let mut max_seen = std::time::Duration::ZERO;
    for _ in 0..20 {
        let d = policy.next_delay();
        if d > max_seen {
            max_seen = d;
        }
    }

    // Max delay is 30s + up to 10% jitter = 33s max
    assert!(
        max_seen <= std::time::Duration::from_millis(33000),
        "Delay should not exceed 33s (30s + 10% jitter), got {max_seen:?}"
    );
}

#[test]
fn test_retry_policy_reset() {
    let mut policy = RetryPolicy::new();

    // Advance several iterations
    for _ in 0..10 {
        let _ = policy.next_delay();
    }

    policy.reset();
    let d = policy.next_delay();

    // Should be back to initial range (100-120ms)
    assert!(
        d.as_millis() < 200,
        "After reset, delay should be near initial, got {}ms",
        d.as_millis()
    );
}

#[test]
fn test_retry_policy_jitter_never_negative() {
    let mut policy = RetryPolicy::new();
    for _ in 0..100 {
        let d = policy.next_delay();
        assert!(
            d > std::time::Duration::ZERO,
            "Delay should always be positive"
        );
    }
}

#[test]
fn test_retry_policy_many_iterations_no_panic() {
    let mut policy = RetryPolicy::new();
    // Should not overflow or panic even after many iterations
    for _ in 0..1000 {
        let d = policy.next_delay();
        assert!(d <= std::time::Duration::from_secs(35));
    }
}

// ─── Signal context tests ───────────────────────────────────────────

fn make_test_context() -> (
    TaskContext,
    mpsc::Sender<TaskSignal>,
    mpsc::Receiver<WorkerRequest>,
) {
    let (request_tx, request_rx) = mpsc::channel::<WorkerRequest>(64);
    let (signal_tx, signal_rx) = mpsc::channel::<TaskSignal>(64);

    let ctx = TaskContext::new(
        "task-1".to_string(),
        "run-1".to_string(),
        "queue".to_string(),
        "test-task".to_string(),
        1,
        "{}".to_string(),
        "{}".to_string(),
        request_tx,
        signal_rx,
    );

    (ctx, signal_tx, request_rx)
}

fn make_signal(id: &str, name: &str, payload: &str) -> TaskSignal {
    TaskSignal {
        signal_id: id.to_string(),
        task_id: "task-1".to_string(),
        signal_name: name.to_string(),
        payload: payload.to_string(),
        timestamp_ms: 1700000000000,
    }
}

#[tokio::test]
async fn test_context_receive_signal() {
    let (mut ctx, signal_tx, _request_rx) = make_test_context();

    signal_tx
        .send(make_signal("sig-1", "approve", r#"{"ok": true}"#))
        .await
        .unwrap();

    let data = ctx.receive_signal().await.expect("Should receive signal");
    assert_eq!(data.signal_id, "sig-1");
    assert_eq!(data.name, "approve");
    assert_eq!(data.payload, r#"{"ok": true}"#);
}

#[tokio::test]
async fn test_context_wait_for_signal_by_name() {
    let (mut ctx, signal_tx, _request_rx) = make_test_context();

    signal_tx.send(make_signal("s-foo", "foo", "1")).await.unwrap();
    signal_tx.send(make_signal("s-bar", "bar", "2")).await.unwrap();

    // Wait for "bar" — should skip "foo" and buffer it
    let bar = ctx.wait_for_signal("bar").await.expect("Should find bar");
    assert_eq!(bar.name, "bar");
    assert_eq!(bar.signal_id, "s-bar");

    // Now receive_signal should return the buffered "foo"
    let foo = ctx.receive_signal().await.expect("Should get buffered foo");
    assert_eq!(foo.name, "foo");
    assert_eq!(foo.signal_id, "s-foo");
}

#[tokio::test]
async fn test_context_wait_for_signal_buffers_non_matching() {
    let (mut ctx, signal_tx, _request_rx) = make_test_context();

    signal_tx.send(make_signal("s1", "alpha", "a")).await.unwrap();
    signal_tx.send(make_signal("s2", "beta", "b")).await.unwrap();
    signal_tx.send(make_signal("s3", "gamma", "c")).await.unwrap();

    // Wait for "gamma" — first two get buffered
    let gamma = ctx.wait_for_signal("gamma").await.expect("Should find gamma");
    assert_eq!(gamma.name, "gamma");

    // Buffered signals should come out in order
    let first = ctx.receive_signal().await.expect("Should get alpha");
    assert_eq!(first.name, "alpha");

    let second = ctx.receive_signal().await.expect("Should get beta");
    assert_eq!(second.name, "beta");
}

#[tokio::test]
async fn test_context_receive_signal_channel_closed() {
    let (mut ctx, signal_tx, _request_rx) = make_test_context();

    // Drop sender to close channel
    drop(signal_tx);

    let result = ctx.receive_signal().await;
    assert!(result.is_none(), "Should return None when channel closed");
}

#[tokio::test]
async fn test_signal_data_parse_payload() {
    let data = SignalData {
        signal_id: "s1".to_string(),
        name: "test".to_string(),
        payload: r#"{"count": 42, "active": true}"#.to_string(),
    };

    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Payload {
        count: i32,
        active: bool,
    }

    let parsed: Payload = data.parse_payload().unwrap();
    assert_eq!(parsed.count, 42);
    assert!(parsed.active);
}

#[tokio::test]
async fn test_signal_data_parse_empty_payload() {
    let data = SignalData {
        signal_id: "s1".to_string(),
        name: "test".to_string(),
        payload: String::new(),
    };

    let result = data.parse_payload::<serde_json::Value>();
    assert!(result.is_err(), "Empty payload should fail to parse");
}

#[tokio::test]
async fn test_context_signal_sends_ack() {
    let (mut ctx, signal_tx, mut request_rx) = make_test_context();

    signal_tx
        .send(make_signal("sig-ack-test", "notify", "{}"))
        .await
        .unwrap();

    let _data = ctx.receive_signal().await.expect("Should receive signal");

    // Verify a SignalAck was sent on the request channel
    let ack_msg = request_rx.recv().await.expect("Should receive ack request");
    match ack_msg.request {
        Some(worker_request::Request::SignalAck(ack)) => {
            assert_eq!(ack.signal_id, "sig-ack-test");
        }
        other => panic!("Expected SignalAck, got {other:?}"),
    }
}
