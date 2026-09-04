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
use valka_wal::{Envelope, FailureOutcome, Lsn, TaskSpec, WalRecord};

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
        v.sort_by(|a, b| b.attempt_number.cmp(&a.attempt_number));
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
        }
    }

    pub fn from_snapshot(snap: ShardSnapshot, lsn: Lsn) -> Self {
        let mut s = Self::new(snap.shard);
        s.shard_seq = snap.shard_seq;
        s.snapshot_seq = snap.shard_seq;
        s.snapshot_lsn = lsn;
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

    pub fn to_snapshot(&self) -> ShardSnapshot {
        ShardSnapshot {
            shard: self.shard,
            shard_seq: self.shard_seq,
            tasks: self.tasks.values().cloned().collect(),
            signals: self.signals.values().cloned().collect(),
            dead_letters: self.dead_letters.values().cloned().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty() && self.signals.is_empty() && self.dead_letters.is_empty()
    }

    fn dl_key(d: &DeadLetterEntry) -> String {
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
    pub fn apply(&mut self, env: &mut Envelope) -> Vec<Transition> {
        if env.shard_seq == 0 {
            self.shard_seq += 1;
            env.shard_seq = self.shard_seq;
        } else {
            if env.shard_seq <= self.shard_seq {
                return Vec::new();
            }
            self.shard_seq = env.shard_seq;
        }
        self.records_since_snapshot += 1;
        let now = env.ts();
        let mut out = Vec::new();

        match &env.record {
            WalRecord::TaskCreated { task } => {
                if self.tasks.contains_key(&task.id) {
                    return out;
                }
                if let Some(k) = &task.idempotency_key {
                    if self.idempotency.contains_key(k) {
                        return out;
                    }
                    self.idempotency.insert(k.clone(), task.id.clone());
                }
                let due = task.scheduled_at.is_none_or(|t| t <= now);
                let st = TaskState {
                    spec: task.clone(),
                    status: TaskStatus::Pending,
                    attempt_count: 0,
                    output: None,
                    error_message: None,
                    due,
                    next_attempt_at: if due { None } else { task.scheduled_at },
                    updated_at: now,
                    runs: Vec::new(),
                    signals: Vec::new(),
                    offered: false,
                };
                out.push(Transition {
                    task_id: task.id.clone(),
                    queue_name: task.queue_name.clone(),
                    from: None,
                    to: Some(TaskStatus::Pending),
                    attempt: 0,
                    worker_id: None,
                    error: None,
                });
                self.tasks.insert(task.id.clone(), st);
            }

            WalRecord::TaskDispatched {
                task_id,
                run_id,
                attempt,
                worker_id,
                node_id,
                lease_until,
            } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if t.status != TaskStatus::Pending || t.runs.iter().any(|r| r.id == *run_id) {
                    return out;
                }
                let from = t.status;
                t.status = TaskStatus::Running;
                t.attempt_count = *attempt;
                t.due = true;
                t.next_attempt_at = None;
                t.offered = false;
                t.updated_at = now;
                t.runs.push(RunState {
                    id: run_id.clone(),
                    attempt_number: *attempt,
                    worker_id: worker_id.clone(),
                    node_id: node_id.clone(),
                    status: RunStatus::Running,
                    output: None,
                    error_message: None,
                    lease_until: *lease_until,
                    started_at: now,
                    completed_at: None,
                    last_heartbeat: now,
                });
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(from),
                    to: Some(TaskStatus::Running),
                    attempt: *attempt,
                    worker_id: Some(worker_id.clone()),
                    error: None,
                });
            }

            WalRecord::RunCompleted {
                task_id,
                run_id,
                output,
            } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if t.status != TaskStatus::Running {
                    return out;
                }
                let Some(run) = t.run_mut(run_id) else {
                    return out;
                };
                if run.status != RunStatus::Running {
                    return out;
                }
                run.status = RunStatus::Completed;
                run.output = output.clone();
                run.completed_at = Some(now);
                let attempt = run.attempt_number;
                let worker = run.worker_id.clone();
                t.status = TaskStatus::Completed;
                t.output = output.clone();
                t.updated_at = now;
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(TaskStatus::Running),
                    to: Some(TaskStatus::Completed),
                    attempt,
                    worker_id: Some(worker),
                    error: None,
                });
            }

            WalRecord::RunFailed {
                task_id,
                run_id,
                error,
                outcome,
            }
            | WalRecord::LeaseExpired {
                task_id,
                run_id,
                outcome,
                error,
            } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if t.status != TaskStatus::Running {
                    return out;
                }
                let Some(run) = t.run_mut(run_id) else {
                    return out;
                };
                if run.status != RunStatus::Running {
                    return out;
                }
                run.status = RunStatus::Failed;
                run.error_message = Some(error.clone());
                run.completed_at = Some(now);
                let attempt = run.attempt_number;
                let worker = run.worker_id.clone();
                t.updated_at = now;
                let to = match outcome {
                    FailureOutcome::Retry { at } => {
                        t.status = TaskStatus::Retry;
                        t.due = false;
                        t.next_attempt_at = Some(*at);
                        t.error_message = Some(error.clone());
                        TaskStatus::Retry
                    }
                    FailureOutcome::Failed => {
                        t.status = TaskStatus::Failed;
                        t.error_message = Some(error.clone());
                        TaskStatus::Failed
                    }
                    FailureOutcome::DeadLetter => {
                        t.status = TaskStatus::DeadLetter;
                        t.error_message = Some(error.clone());
                        let entry = DeadLetterEntry {
                            id: uuid::Uuid::now_v7().to_string(),
                            task_id: task_id.clone(),
                            queue_name: t.spec.queue_name.clone(),
                            task_name: t.spec.task_name.clone(),
                            input: t.spec.input.clone(),
                            error_message: Some(error.clone()),
                            attempt_count: t.attempt_count,
                            metadata: t.spec.metadata.clone(),
                            created_at: now,
                        };
                        self.dead_letters.insert(Self::dl_key(&entry), entry);
                        TaskStatus::DeadLetter
                    }
                };
                let t = &self.tasks[task_id];
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(TaskStatus::Running),
                    to: Some(to),
                    attempt,
                    worker_id: Some(worker),
                    error: Some(error.clone()),
                });
            }

            WalRecord::LeaseExtended {
                task_id,
                run_id,
                lease_until,
            } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if let Some(run) = t.run_mut(run_id)
                    && run.status == RunStatus::Running
                {
                    run.lease_until = run.lease_until.max(*lease_until);
                    run.last_heartbeat = now;
                }
            }

            WalRecord::TaskPromoted { task_id } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                let from = t.status;
                let was_runnable = t.status == TaskStatus::Pending && t.due;
                match t.status {
                    TaskStatus::Retry | TaskStatus::Pending => {
                        t.status = TaskStatus::Pending;
                        t.due = true;
                        t.next_attempt_at = None;
                        t.updated_at = now;
                    }
                    _ => return out,
                }
                if !was_runnable {
                    out.push(Transition {
                        task_id: task_id.clone(),
                        queue_name: t.spec.queue_name.clone(),
                        from: Some(from),
                        to: Some(TaskStatus::Pending),
                        attempt: t.attempt_count,
                        worker_id: None,
                        error: None,
                    });
                }
            }

            WalRecord::TaskCancelled { task_id, reason } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if t.is_terminal() {
                    return out;
                }
                let from = t.status;
                let worker = t
                    .current_run()
                    .filter(|r| r.status == RunStatus::Running)
                    .map(|r| r.worker_id.clone());
                if let Some(run) = t.current_run_mut()
                    && run.status == RunStatus::Running
                {
                    run.status = RunStatus::Failed;
                    run.error_message = Some(reason.clone());
                    run.completed_at = Some(now);
                }
                t.status = TaskStatus::Cancelled;
                t.error_message = Some(reason.clone());
                t.due = true;
                t.next_attempt_at = None;
                t.offered = false;
                t.updated_at = now;
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(from),
                    to: Some(TaskStatus::Cancelled),
                    attempt: t.attempt_count,
                    worker_id: worker,
                    error: None,
                });
            }

            WalRecord::TaskDeleted { task_id } => {
                let Some(t) = self.tasks.remove(task_id) else {
                    return out;
                };
                if let Some(k) = &t.spec.idempotency_key {
                    self.idempotency.remove(k);
                }
                for sid in &t.signals {
                    self.signals.remove(sid);
                }
                self.dead_letters.retain(|_, d| d.task_id != *task_id);
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(t.status),
                    to: None,
                    attempt: t.attempt_count,
                    worker_id: None,
                    error: None,
                });
            }

            WalRecord::SignalCreated {
                signal_id,
                task_id,
                signal_name,
                payload,
            } => {
                if self.signals.contains_key(signal_id) {
                    return out;
                }
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                t.signals.push(signal_id.clone());
                self.signals.insert(
                    signal_id.clone(),
                    SignalState {
                        id: signal_id.clone(),
                        task_id: task_id.clone(),
                        signal_name: signal_name.clone(),
                        payload: payload.clone(),
                        status: SignalStatus::Pending,
                        created_at: now,
                        delivered_at: None,
                        acknowledged_at: None,
                    },
                );
            }

            WalRecord::SignalDelivered { signal_id, .. } => {
                if let Some(s) = self.signals.get_mut(signal_id)
                    && s.status == SignalStatus::Pending
                {
                    s.status = SignalStatus::Delivered;
                    s.delivered_at = Some(now);
                }
            }

            WalRecord::SignalAcked { signal_id, .. } => {
                if let Some(s) = self.signals.get_mut(signal_id)
                    && s.status == SignalStatus::Delivered
                {
                    s.status = SignalStatus::Acknowledged;
                    s.acknowledged_at = Some(now);
                }
            }

            WalRecord::SignalsReset { task_id } => {
                for s in self.signals.values_mut() {
                    if s.task_id == *task_id && s.status == SignalStatus::Delivered {
                        s.status = SignalStatus::Pending;
                        s.delivered_at = None;
                    }
                }
            }

            WalRecord::ShardCleared => {
                for (_, t) in self.tasks.drain() {
                    out.push(Transition {
                        task_id: t.spec.id,
                        queue_name: t.spec.queue_name,
                        from: Some(t.status),
                        to: None,
                        attempt: t.attempt_count,
                        worker_id: None,
                        error: None,
                    });
                }
                self.signals.clear();
                self.idempotency.clear();
                self.dead_letters.clear();
            }
        }
        out
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str) -> TaskSpec {
        TaskSpec {
            id: id.into(),
            queue_name: "q".into(),
            task_name: "t".into(),
            input: None,
            priority: 0,
            max_retries: 3,
            timeout_seconds: 30,
            idempotency_key: None,
            metadata: serde_json::json!({}),
            scheduled_at: None,
            created_at: Utc::now(),
        }
    }

    fn env(rec: WalRecord) -> Envelope {
        Envelope::new(ShardId(0), rec)
    }

    #[test]
    fn full_lifecycle_and_idempotent_replay() {
        let mut s = ShardState::new(ShardId(0));
        let mut recs = vec![
            env(WalRecord::TaskCreated { task: spec("a") }),
            env(WalRecord::TaskDispatched {
                task_id: "a".into(),
                run_id: "r1".into(),
                attempt: 1,
                worker_id: "w".into(),
                node_id: "n".into(),
                lease_until: Utc::now(),
            }),
            env(WalRecord::RunCompleted {
                task_id: "a".into(),
                run_id: "r1".into(),
                output: Some(serde_json::json!({"ok": true})),
            }),
        ];
        let mut transitions = Vec::new();
        for r in recs.iter_mut() {
            transitions.extend(s.apply(r));
        }
        assert_eq!(s.shard_seq, 3);
        assert_eq!(s.tasks["a"].status, TaskStatus::Completed);
        assert_eq!(transitions.len(), 3);
        assert_eq!(transitions[2].to, Some(TaskStatus::Completed));

        // Replaying the same records (with their shard_seq set) is a no-op.
        let before = s.tasks["a"].clone();
        for r in recs.iter_mut() {
            assert!(s.apply(r).is_empty());
        }
        assert_eq!(s.tasks["a"], before);
        assert_eq!(s.shard_seq, 3);
    }

    #[test]
    fn retry_then_dead_letter() {
        let mut s = ShardState::new(ShardId(0));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("a") }));
        s.apply(&mut env(WalRecord::TaskDispatched {
            task_id: "a".into(),
            run_id: "r1".into(),
            attempt: 1,
            worker_id: "w".into(),
            node_id: "n".into(),
            lease_until: Utc::now(),
        }));
        let at = Utc::now() + chrono::Duration::seconds(5);
        let tr = s.apply(&mut env(WalRecord::RunFailed {
            task_id: "a".into(),
            run_id: "r1".into(),
            error: "boom".into(),
            outcome: FailureOutcome::Retry { at },
        }));
        assert_eq!(tr[0].to, Some(TaskStatus::Retry));
        assert!(!s.tasks["a"].due);
        assert_eq!(s.tasks["a"].next_attempt_at, Some(at));

        let tr = s.apply(&mut env(WalRecord::TaskPromoted {
            task_id: "a".into(),
        }));
        assert_eq!(tr[0].to, Some(TaskStatus::Pending));
        assert!(s.tasks["a"].is_runnable());

        s.apply(&mut env(WalRecord::TaskDispatched {
            task_id: "a".into(),
            run_id: "r2".into(),
            attempt: 2,
            worker_id: "w".into(),
            node_id: "n".into(),
            lease_until: Utc::now(),
        }));
        s.apply(&mut env(WalRecord::LeaseExpired {
            task_id: "a".into(),
            run_id: "r2".into(),
            error: "lease expired".into(),
            outcome: FailureOutcome::DeadLetter,
        }));
        assert_eq!(s.tasks["a"].status, TaskStatus::DeadLetter);
        assert_eq!(s.dead_letters.len(), 1);
        assert_eq!(s.tasks["a"].runs.len(), 2);

        // Deleting the task removes its DLQ entry.
        s.apply(&mut env(WalRecord::TaskDeleted {
            task_id: "a".into(),
        }));
        assert!(s.tasks.is_empty() && s.dead_letters.is_empty());
    }

    #[test]
    fn stale_completion_after_cancel_is_noop() {
        let mut s = ShardState::new(ShardId(0));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("a") }));
        s.apply(&mut env(WalRecord::TaskDispatched {
            task_id: "a".into(),
            run_id: "r1".into(),
            attempt: 1,
            worker_id: "w".into(),
            node_id: "n".into(),
            lease_until: Utc::now(),
        }));
        let tr = s.apply(&mut env(WalRecord::TaskCancelled {
            task_id: "a".into(),
            reason: "user".into(),
        }));
        assert_eq!(tr[0].worker_id.as_deref(), Some("w"));
        assert!(
            s.apply(&mut env(WalRecord::RunCompleted {
                task_id: "a".into(),
                run_id: "r1".into(),
                output: None,
            }))
            .is_empty()
        );
        assert_eq!(s.tasks["a"].status, TaskStatus::Cancelled);
    }

    #[test]
    fn idempotency_key_dedupes_within_shard() {
        let mut s = ShardState::new(ShardId(0));
        let mut a = spec("a");
        a.idempotency_key = Some("k".into());
        let mut b = spec("b");
        b.idempotency_key = Some("k".into());
        assert_eq!(
            s.apply(&mut env(WalRecord::TaskCreated { task: a })).len(),
            1
        );
        assert!(
            s.apply(&mut env(WalRecord::TaskCreated { task: b }))
                .is_empty()
        );
        assert_eq!(s.tasks.len(), 1);
    }

    #[test]
    fn snapshot_round_trip_preserves_state() {
        let mut s = ShardState::new(ShardId(3));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("a") }));
        s.apply(&mut env(WalRecord::SignalCreated {
            signal_id: "sig".into(),
            task_id: "a".into(),
            signal_name: "ping".into(),
            payload: None,
        }));
        let snap = s.to_snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let back: ShardSnapshot = serde_json::from_str(&json).unwrap();
        let r = ShardState::from_snapshot(back, Lsn::new(1, 4));
        assert_eq!(r.shard_seq, 2);
        assert_eq!(r.tasks["a"].signals, vec!["sig".to_string()]);
        assert_eq!(r.signals["sig"].status, SignalStatus::Pending);
        assert_eq!(r.snapshot_lsn, Lsn::new(1, 4));
    }

    #[test]
    fn eviction_drops_only_old_terminal_tasks() {
        let mut s = ShardState::new(ShardId(0));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("live") }));
        let mut done = env(WalRecord::TaskCreated { task: spec("done") });
        done.ts_ms = 1000;
        s.apply(&mut done);
        let mut c = env(WalRecord::TaskCancelled {
            task_id: "done".into(),
            reason: "x".into(),
        });
        c.ts_ms = 2000;
        s.apply(&mut c);
        assert_eq!(s.evict_terminal_before(Utc::now()), 1);
        assert!(s.tasks.contains_key("live"));
    }
}
