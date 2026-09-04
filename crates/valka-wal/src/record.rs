//! The WAL record set. Every state transition in Valka is one of these.
//!
//! Records are JSON inside zstd-compressed segments (see `segment.rs`). Field names are
//! part of the on-disk format: add fields with `#[serde(default)]`, never rename.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use valka_core::ShardId;

/// Immutable definition of a task as submitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: String,
    pub queue_name: String,
    pub task_name: String,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    #[serde(default)]
    pub priority: i32,
    pub max_retries: i32,
    pub timeout_seconds: i32,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default = "empty_object")]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub scheduled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

fn lease_expired_msg() -> String {
    "Lease expired".to_string()
}

/// What the engine decided to do about a failed / expired run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FailureOutcome {
    /// Try again at `at`.
    Retry { at: DateTime<Utc> },
    /// Non-retryable failure.
    Failed,
    /// Max retries exhausted.
    DeadLetter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WalRecord {
    TaskCreated {
        task: TaskSpec,
    },
    TaskDispatched {
        task_id: String,
        run_id: String,
        attempt: i32,
        worker_id: String,
        node_id: String,
        lease_until: DateTime<Utc>,
    },
    RunCompleted {
        task_id: String,
        run_id: String,
        #[serde(default)]
        output: Option<serde_json::Value>,
    },
    RunFailed {
        task_id: String,
        run_id: String,
        error: String,
        outcome: FailureOutcome,
    },
    LeaseExpired {
        task_id: String,
        run_id: String,
        #[serde(default = "lease_expired_msg")]
        error: String,
        outcome: FailureOutcome,
    },
    LeaseExtended {
        task_id: String,
        run_id: String,
        lease_until: DateTime<Utc>,
    },
    TaskPromoted {
        task_id: String,
    },
    TaskCancelled {
        task_id: String,
        reason: String,
    },
    TaskDeleted {
        task_id: String,
    },
    SignalCreated {
        signal_id: String,
        task_id: String,
        signal_name: String,
        #[serde(default)]
        payload: Option<serde_json::Value>,
    },
    SignalDelivered {
        signal_id: String,
        task_id: String,
    },
    SignalAcked {
        signal_id: String,
        task_id: String,
    },
    SignalsReset {
        task_id: String,
    },
    /// Drop every task in the shard (REST `DELETE /api/v1/tasks`).
    ShardCleared,
}

impl WalRecord {
    /// Task this record concerns, if any (used for dedupe and diagnostics).
    pub fn task_id(&self) -> Option<&str> {
        use WalRecord::*;
        match self {
            TaskCreated { task } => Some(&task.id),
            TaskDispatched { task_id, .. }
            | RunCompleted { task_id, .. }
            | RunFailed { task_id, .. }
            | LeaseExpired { task_id, .. }
            | LeaseExtended { task_id, .. }
            | TaskPromoted { task_id }
            | TaskCancelled { task_id, .. }
            | TaskDeleted { task_id }
            | SignalCreated { task_id, .. }
            | SignalDelivered { task_id, .. }
            | SignalAcked { task_id, .. }
            | SignalsReset { task_id } => Some(task_id),
            ShardCleared => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        use WalRecord::*;
        match self {
            TaskCreated { .. } => "task_created",
            TaskDispatched { .. } => "task_dispatched",
            RunCompleted { .. } => "run_completed",
            RunFailed { .. } => "run_failed",
            LeaseExpired { .. } => "lease_expired",
            LeaseExtended { .. } => "lease_extended",
            TaskPromoted { .. } => "task_promoted",
            TaskCancelled { .. } => "task_cancelled",
            TaskDeleted { .. } => "task_deleted",
            SignalCreated { .. } => "signal_created",
            SignalDelivered { .. } => "signal_delivered",
            SignalAcked { .. } => "signal_acked",
            SignalsReset { .. } => "signals_reset",
            ShardCleared => "shard_cleared",
        }
    }
}

/// A record plus routing / identity metadata. This is the unit stored in a segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub shard: ShardId,
    /// Per-shard monotonic sequence assigned when the record is applied. Snapshots record
    /// the last applied `shard_seq`, so replay knows exactly which records are already
    /// reflected without relying on idempotency.
    #[serde(default)]
    pub shard_seq: u64,
    /// UUIDv7; unique per record, used for dedupe on retry.
    pub record_id: String,
    /// Wall-clock at production time. Informational; replay never consults the clock.
    pub ts_ms: i64,
    #[serde(flatten)]
    pub record: WalRecord,
}

impl Envelope {
    pub fn new(shard: ShardId, record: WalRecord) -> Self {
        Self {
            shard,
            shard_seq: 0,
            record_id: uuid::Uuid::now_v7().to_string(),
            ts_ms: Utc::now().timestamp_millis(),
            record,
        }
    }

    pub fn with_ts(shard: ShardId, ts: DateTime<Utc>, record: WalRecord) -> Self {
        Self {
            shard,
            shard_seq: 0,
            record_id: uuid::Uuid::now_v7().to_string(),
            ts_ms: ts.timestamp_millis(),
            record,
        }
    }

    pub fn ts(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.ts_ms).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape_is_flat_and_tagged() {
        let e = Envelope::new(
            ShardId(7),
            WalRecord::TaskPromoted {
                task_id: "t".into(),
            },
        );
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "task_promoted");
        assert_eq!(v["shard"], 7);
        assert_eq!(v["task_id"], "t");
        let back: Envelope = serde_json::from_value(v).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let j = r#"{"shard":1,"record_id":"r","ts_ms":1,"type":"task_deleted","task_id":"x","future_field":true}"#;
        let e: Envelope = serde_json::from_str(j).unwrap();
        assert_eq!(e.record.task_id(), Some("x"));
    }
}
