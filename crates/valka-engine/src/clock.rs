//! Wall-clock abstraction. The default derives wall time from tokio's instant so that
//! `tokio::time::pause()` + `advance()` moves *both* timers and timestamps in tests.

use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct TokioClock {
    base_wall: DateTime<Utc>,
    base_instant: tokio::time::Instant,
}

impl TokioClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            base_wall: Utc::now(),
            base_instant: tokio::time::Instant::now(),
        })
    }

    /// Start the clock at a fixed wall time (deterministic tests).
    pub fn starting_at(base_wall: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self {
            base_wall,
            base_instant: tokio::time::Instant::now(),
        })
    }
}

impl Clock for TokioClock {
    fn now(&self) -> DateTime<Utc> {
        let elapsed = self.base_instant.elapsed();
        self.base_wall + Duration::milliseconds(elapsed.as_millis() as i64)
    }
}

/// Plain system clock, for callers outside a tokio runtime.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
