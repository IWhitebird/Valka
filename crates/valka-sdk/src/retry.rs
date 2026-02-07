use std::time::Duration;

/// Exponential backoff with jitter for reconnection
pub struct RetryPolicy {
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
    attempt: u32,
}

impl RetryPolicy {
    pub fn new() -> Self {
        Self {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
            attempt: 0,
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        let delay_ms =
            self.initial_delay.as_millis() as f64 * self.multiplier.powi(self.attempt as i32);
        let capped = delay_ms.min(self.max_delay.as_millis() as f64);

        // Add jitter (10%)
        let jitter = capped * 0.1 * rand_factor();
        let total = Duration::from_millis((capped + jitter) as u64);

        self.attempt += 1;
        total
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

fn rand_factor() -> f64 {
    // Simple pseudo-random using time
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 1000) as f64 / 1000.0
}
