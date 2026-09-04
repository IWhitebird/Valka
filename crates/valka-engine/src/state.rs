//! Per-shard state and the pure `apply` function.
//!
//! `apply` is the *only* way state changes, at runtime and during replay alike. It is
//! total: a record whose precondition no longer holds is a no-op, never a panic. It
//! returns the observable transitions so the engine can drive timers, events and the
//! pending index without re-deriving them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use valka_core::{ShardId, TaskStatus};
use valka_wal::{Lsn, TaskSpec};

use crate::view::{DeadLetterView, RunView, SignalView, TaskView};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Running => "RUNNING",
            RunStatus::Completed => "COMPLETED",
            RunStatus::Failed => "FAILED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub id: String,
    pub attempt_number: i32,
    pub worker_id: String,
    pub node_id: String,
    pub status: RunStatus,
    pub output: Option<Value>,
    pub error_message: Option<String>,
    pub lease_until: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_heartbeat: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalStatus {
    Pending,
    Delivered,
    Acknowledged,
}

impl SignalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SignalStatus::Pending => "PENDING",
            SignalStatus::Delivered => "DELIVERED",
            SignalStatus::Acknowledged => "ACKNOWLEDGED",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "PENDING" => Some(Self::Pending),
            "DELIVERED" => Some(Self::Delivered),
            "ACKNOWLEDGED" => Some(Self::Acknowledged),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalState {
    pub id: String,
    pub task_id: String,
    pub signal_name: String,
    pub payload: Option<Value>,
    pub status: SignalStatus,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub acknowledged_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    pub id: String,
    pub task_id: String,
    pub queue_name: String,
    pub task_name: String,
    pub input: Option<Value>,
    pub error_message: Option<String>,
    pub attempt_count: i32,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskState {
    pub spec: TaskSpec,
    pub status: TaskStatus,
    pub attempt_count: i32,
    pub output: Option<Value>,
    pub error_message: Option<String>,
    /// For PENDING: whether the task may be dispatched now (false while `scheduled_at` or
    /// a retry backoff is in the future). Always true for non-PENDING states.
    pub due: bool,
    /// When a not-due PENDING/RETRY task becomes due.
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub runs: Vec<RunState>,
    pub signals: Vec<String>,
    /// Runtime-only: the task has been handed to the matching layer and must not be
    /// offered again until it leaves PENDING or is explicitly un-offered.
    #[serde(skip)]
    pub offered: bool,
}

impl TaskState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            TaskStatus::Completed
                | TaskStatus::Failed
                | TaskStatus::DeadLetter
                | TaskStatus::Cancelled
        )
    }

    pub fn is_runnable(&self) -> bool {
        self.status == TaskStatus::Pending && self.due && !self.offered
    }

    pub fn current_run(&self) -> Option<&RunState> {
        self.runs.last()
    }

    pub fn current_run_mut(&mut self) -> Option<&mut RunState> {
        self.runs.last_mut()
    }

    pub fn run_mut(&mut self, run_id: &str) -> Option<&mut RunState> {
        self.runs.iter_mut().find(|r| r.id == run_id)
    }

    pub fn view(&self) -> TaskView {
        TaskView {
            id: self.spec.id.clone(),
            queue_name: self.spec.queue_name.clone(),
            task_name: self.spec.task_name.clone(),
            status: self.status,
            priority: self.spec.priority,
            max_retries: self.spec.max_retries,
            attempt_count: self.attempt_count,
            timeout_seconds: self.spec.timeout_seconds,
            idempotency_key: self.spec.idempotency_key.clone(),
            input: self.spec.input.clone(),
            metadata: self.spec.metadata.clone(),
            output: self.output.clone(),
            error_message: self.error_message.clone(),
            scheduled_at: if self.status == TaskStatus::Retry
                || (self.status == TaskStatus::Pending && !self.due)
            {
                self.next_attempt_at.or(self.spec.scheduled_at)
            } else {
                self.spec.scheduled_at
            },
            created_at: self.spec.created_at,
            updated_at: self.updated_at,
        }
    }

    pub fn run_views(&self) -> Vec<RunView> {
        let mut v: Vec<RunView> = self
            .runs
            .iter()
            .map(|r| RunView {
                id: r.id.clone(),
                task_id: self.spec.id.clone(),
                attempt_number: r.attempt_number,
                worker_id: r.worker_id.clone(),
                assigned_node_id: r.node_id.clone(),
                status: r.status.as_str().to_string(),
                output: r.output.clone(),
                error_message: r.error_message.clone(),
                lease_expires_at: r.lease_until,
                started_at: r.started_at,
                completed_at: r.completed_at,
                last_heartbeat: r.last_heartbeat,
            })
            .collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.attempt_number));
        v
    }
}

impl SignalState {
    pub fn view(&self) -> SignalView {
        SignalView {
            id: self.id.clone(),
            task_id: self.task_id.clone(),
            signal_name: self.signal_name.clone(),
            payload: self.payload.clone(),
            status: self.status.as_str().to_string(),
            created_at: self.created_at,
            delivered_at: self.delivered_at,
            acknowledged_at: self.acknowledged_at,
        }
    }
}

impl DeadLetterEntry {
    pub fn view(&self) -> DeadLetterView {
        DeadLetterView {
            id: self.id.clone(),
            task_id: self.task_id.clone(),
            queue_name: self.queue_name.clone(),
            task_name: self.task_name.clone(),
            input: self.input.clone(),
            error_message: self.error_message.clone(),
            attempt_count: self.attempt_count,
            metadata: self.metadata.clone(),
            created_at: self.created_at,
        }
    }
}

/// A status change observed while applying a record.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub task_id: String,
    pub queue_name: String,
    pub from: Option<TaskStatus>,
    pub to: Option<TaskStatus>,
    pub attempt: i32,
    pub worker_id: Option<String>,
    pub error: Option<String>,
}

/// Snapshot payload for one shard. Everything except runtime-only fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardSnapshot {
    pub shard: ShardId,
    pub shard_seq: u64,
    #[serde(default)]
    pub taken_at: Option<DateTime<Utc>>,
    pub tasks: Vec<TaskState>,
    pub signals: Vec<SignalState>,
    pub dead_letters: Vec<DeadLetterEntry>,
}

/// The in-RAM state of one shard.
#[derive(Debug)]
pub struct ShardState {
    pub shard: ShardId,
    /// Last applied per-shard sequence number.
    pub shard_seq: u64,
    pub tasks: HashMap<String, TaskState>,
    pub signals: HashMap<String, SignalState>,
    pub idempotency: HashMap<String, String>,
    /// Ordered newest-first by insertion (BTreeMap on a reverse key).
    pub dead_letters: BTreeMap<String, DeadLetterEntry>,
    /// Records applied since the last snapshot.
    pub records_since_snapshot: u64,
    /// `shard_seq` covered by the newest snapshot in the bucket.
    pub snapshot_seq: u64,
    /// Writer LSN recorded with the newest snapshot (for segment truncation).
    pub snapshot_lsn: Lsn,
    /// LSN of the segment holding this shard's oldest record not yet covered by a
    /// snapshot. Segments below the minimum over all dirty shards can be truncated.
    pub dirty_since_lsn: Option<Lsn>,
    /// When the newest snapshot was taken.
    pub snapshot_at: Option<DateTime<Utc>>,
}

impl ShardState {
    pub fn new(shard: ShardId) -> Self {
        Self {
            shard,
            shard_seq: 0,
            tasks: HashMap::new(),
            signals: HashMap::new(),
            idempotency: HashMap::new(),
            dead_letters: BTreeMap::new(),
            records_since_snapshot: 0,
            snapshot_seq: 0,
            snapshot_lsn: Lsn::ZERO,
            dirty_since_lsn: None,
            snapshot_at: None,
        }
    }

    pub fn from_snapshot(snap: ShardSnapshot, lsn: Lsn) -> Self {
        let mut s = Self::new(snap.shard);
        s.shard_seq = snap.shard_seq;
        s.snapshot_seq = snap.shard_seq;
        s.snapshot_lsn = lsn;
        s.snapshot_at = snap.taken_at;
        for t in snap.tasks {
            if let Some(k) = &t.spec.idempotency_key {
                s.idempotency.insert(k.clone(), t.spec.id.clone());
            }
            s.tasks.insert(t.spec.id.clone(), t);
        }
        for sig in snap.signals {
            s.signals.insert(sig.id.clone(), sig);
        }
        for d in snap.dead_letters {
            s.dead_letters.insert(Self::dl_key(&d), d);
        }
        s
    }

    pub fn to_snapshot(&self, taken_at: DateTime<Utc>) -> ShardSnapshot {
        ShardSnapshot {
            shard: self.shard,
            shard_seq: self.shard_seq,
            taken_at: Some(taken_at),
            tasks: self.tasks.values().cloned().collect(),
            signals: self.signals.values().cloned().collect(),
            dead_letters: self.dead_letters.values().cloned().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty() && self.signals.is_empty() && self.dead_letters.is_empty()
    }

    pub(crate) fn dl_key(d: &DeadLetterEntry) -> String {
        // Reverse-chronological: newer entries sort first.
        format!(
            "{:020}-{}",
            i64::MAX - d.created_at.timestamp_millis(),
            d.id
        )
    }

    /// Apply a record. Assigns `shard_seq` when the envelope carries none (runtime path);
    /// during replay the envelope's own `shard_seq` is trusted and records at or below the
    /// current sequence are skipped.
    /// Drop terminal tasks older than `cutoff`. Not a WAL operation: it is a cache
    /// eviction re-applied identically after recovery.
    pub fn evict_terminal_before(&mut self, cutoff: DateTime<Utc>) -> usize {
        let victims: Vec<String> = self
            .tasks
            .values()
            .filter(|t| t.is_terminal() && t.updated_at < cutoff)
            .map(|t| t.spec.id.clone())
            .collect();
        for id in &victims {
            if let Some(t) = self.tasks.remove(id) {
                if let Some(k) = &t.spec.idempotency_key {
                    self.idempotency.remove(k);
                }
                for sid in &t.signals {
                    self.signals.remove(sid);
                }
            }
        }
        victims.len()
    }
}
