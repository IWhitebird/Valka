use valka_scheduler::retry::compute_retry_delay;

#[test]
fn test_exponential_backoff() {
    let base_delay = 1;
    let max_delay = 3600;

    let d0 = compute_retry_delay(0, base_delay, max_delay);
    let d1 = compute_retry_delay(1, base_delay, max_delay);
    let d2 = compute_retry_delay(2, base_delay, max_delay);
    let d3 = compute_retry_delay(3, base_delay, max_delay);

    assert_eq!(d0.num_seconds(), 1);
    assert_eq!(d1.num_seconds(), 2);
    assert_eq!(d2.num_seconds(), 4);
    assert_eq!(d3.num_seconds(), 8);
}

#[test]
fn test_max_delay_cap() {
    let base_delay = 1;
    let max_delay = 60;

    let d10 = compute_retry_delay(10, base_delay, max_delay);
    assert_eq!(d10.num_seconds(), 60, "Should be capped at max_delay");
}

#[test]
fn test_large_attempt_count_no_overflow() {
    let base_delay = 1;
    let max_delay = 3600;

    // Should not panic from overflow
    let d = compute_retry_delay(100, base_delay, max_delay);
    assert_eq!(d.num_seconds(), 3600, "Should be capped at max_delay");
}

#[test]
fn test_sdk_retry_policy() {
    let mut policy = valka_sdk::retry::RetryPolicy::new();

    let d1 = policy.next_delay();
    assert!(
        d1.as_millis() >= 100 && d1.as_millis() <= 200,
        "First delay should be around 100ms, got {}ms",
        d1.as_millis()
    );

    let _d2 = policy.next_delay();
    let _d3 = policy.next_delay();

    // After reset
    policy.reset();
    let d_reset = policy.next_delay();
    assert!(
        d_reset.as_millis() < 200,
        "Should be close to initial delay after reset"
    );
}

#[test]
fn test_sdk_retry_policy_max_cap() {
    let mut policy = valka_sdk::retry::RetryPolicy::new();

    // Run many iterations - should never exceed max_delay (30s)
    let mut max_seen = std::time::Duration::ZERO;
    for _ in 0..20 {
        let d = policy.next_delay();
        if d > max_seen {
            max_seen = d;
        }
    }

    assert!(
        max_seen <= std::time::Duration::from_secs(35),
        "Delay should not exceed max_delay + jitter"
    );
}
