use chrono::Duration;

/// Exponential backoff: `base * 2^attempt`, capped.
pub fn compute_retry_delay(
    attempt_count: i32,
    base_delay_secs: u64,
    max_delay_secs: u64,
) -> Duration {
    let delay_secs =
        base_delay_secs.saturating_mul(2u64.saturating_pow(attempt_count.max(0) as u32));
    let capped = delay_secs.min(max_delay_secs);
    Duration::seconds(capped as i64)
}
