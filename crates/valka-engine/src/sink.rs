//! How the engine hands runnable tasks to the matching layer.

use serde_json::Value;

/// Everything a worker needs to start a task, minus the run id (assigned at dispatch).
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchableTask {
    pub task_id: String,
    pub queue_name: String,
    pub task_name: String,
    pub input: Option<Value>,
    /// The attempt number this dispatch *will* be.
    pub attempt_number: i32,
    pub timeout_seconds: i32,
    pub metadata: Value,
    pub priority: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferOutcome {
    /// Handed to a waiting worker (dispatch will follow).
    Matched,
    /// Parked in the matching buffer; dispatch will follow when a worker frees up.
    Buffered,
    /// Buffer full. The engine keeps the task in its pending index for the feeder.
    Rejected,
}

pub trait TaskSink: Send + Sync {
    fn offer(&self, task: DispatchableTask) -> OfferOutcome;
    /// Free capacity for a queue, so the feeder does not over-offer.
    fn capacity(&self, queue_name: &str) -> usize;
}

/// Sink that never accepts; tasks stay pending until something calls `take_pending`.
pub struct NoopSink;

impl TaskSink for NoopSink {
    fn offer(&self, _task: DispatchableTask) -> OfferOutcome {
        OfferOutcome::Rejected
    }
    fn capacity(&self, _queue_name: &str) -> usize {
        0
    }
}
