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
