//! The engine: command API, recovery, and background loops.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures::StreamExt;
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, broadcast, watch};
use tracing::{info, warn};
use valka_core::{SchedulerConfig, ServerError, ShardId, TaskStatus, WalConfig, shard_of_task_id};
use valka_wal::lsn::keys;
use valka_wal::ownership::{Ownership, OwnershipCheck};
use valka_wal::{
    Durable, Envelope, FailureOutcome, Lsn, Store, TaskSpec, WalRecord, WalWriter, WalWriterConfig,
    reader, snapshot,
};

use crate::clock::{Clock, TokioClock};
use crate::retry::compute_retry_delay;
use crate::sink::{DispatchableTask, NoopSink, OfferOutcome, TaskSink};
use crate::state::{RunStatus, ShardSnapshot, ShardState, SignalStatus, Transition};
use crate::timers::{TimerKind, TimerWheel};
use crate::view::{DeadLetterView, RunView, SignalView, TaskView};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub node_id: String,
    pub wal: WalConfig,
    pub scheduler: SchedulerConfig,
    /// Feeder cadence (from `MatchingConfig`).
    pub feeder_interval: Duration,
    pub feeder_batch_size: usize,
    /// Skip the bucket-backed ownership protocol (tests with `SingleNodeOwnership`).
    pub trust_self: bool,
}

impl EngineConfig {
    pub fn for_tests(node_id: &str) -> Self {
        Self {
            node_id: node_id.to_string(),
            wal: WalConfig {
                flush_interval_ms: 5,
                snapshot_interval_secs: 3600,
                ..Default::default()
            },
            scheduler: SchedulerConfig {
                timer_tick_ms: 10,
                ..Default::default()
            },
            feeder_interval: Duration::from_millis(10),
            feeder_batch_size: 100,
            trust_self: true,
        }
    }
}

/// A status transition, broadcast after it is durable.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineEvent {
    pub task_id: String,
    pub queue_name: String,
    pub previous: Option<TaskStatus>,
    pub new: Option<TaskStatus>,
    pub worker_id: Option<String>,
    pub node_id: String,
    pub attempt: i32,
    pub error: Option<String>,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreateTask {
    pub queue_name: String,
    pub task_name: String,
    pub input: Option<Value>,
    pub priority: i32,
    pub max_retries: i32,
    pub timeout_seconds: i32,
    pub idempotency_key: Option<String>,
    pub metadata: Value,
    pub scheduled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct DispatchInfo {
    pub run_id: String,
    pub attempt: i32,
    pub lease_until: DateTime<Utc>,
    pub task: DispatchableTask,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FailResult {
    pub outcome: FailureOutcome,
}

/// Runnable tasks ordered by priority desc, then creation order.
pub(crate) type PendingKey = (i32, i64, String);
/// task_id -> (shard, run_id, lease_until) awaiting a coalesced `LeaseExtended` record.
pub(crate) type LeaseDirty = HashMap<String, (ShardId, String, DateTime<Utc>)>;
type LoadedSnapshot = Result<Option<(ShardId, Lsn, ShardSnapshot)>, ServerError>;

pub(crate) struct Inner {
    pub(crate) shards: Vec<Mutex<ShardState>>,
    pub(crate) writer: WalWriter,
    pub(crate) store: Store,
    pub(crate) ownership: Option<Arc<Ownership>>,
    pub(crate) cfg: EngineConfig,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) timers: Mutex<TimerWheel>,
    pub(crate) pending: Mutex<BTreeMap<String, BTreeSet<PendingKey>>>,
    pub(crate) lease_dirty: Mutex<LeaseDirty>,
    pub(crate) events: broadcast::Sender<EngineEvent>,
    pub(crate) sink: RwLock<Arc<dyn TaskSink>>,
    pub(crate) pending_wake: Notify,
    pub(crate) shutdown: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

impl Engine {
    /// Open (or create) the engine against a bucket: claim ownership, load snapshots,
    /// replay the WAL, start background loops.
    pub async fn open(store: Store, cfg: EngineConfig) -> Result<Self, ServerError> {
        Self::open_with(store, cfg, TokioClock::new(), Arc::new(NoopSink)).await
    }

    pub async fn open_with(
        store: Store,
        cfg: EngineConfig,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn TaskSink>,
    ) -> Result<Self, ServerError> {
        let node_id = cfg.node_id.clone();
        let (ownership, epoch): (Option<Arc<Ownership>>, u32) = if cfg.trust_self {
            let last = reader::list_segments(&store, &node_id, None)
                .await?
                .last()
                .map(|(l, _)| l.epoch)
                .unwrap_or(0);
            (None, last + 1)
        } else {
            let o = Ownership::claim_all(store.clone(), &node_id).await?;
            let e = o.epoch();
            (Some(o), e)
        };

        // 1. Snapshots: one LIST, then parallel GETs of the newest per shard.
        let started = std::time::Instant::now();
        let mut shards: Vec<ShardState> = ShardId::all().map(ShardState::new).collect();
        let all_snaps = store.list("snapshots/").await?;
        let snapshotted: BTreeSet<ShardId> = all_snaps
            .iter()
            .filter(|(key, _)| keys::snapshot_lsn(key).is_some())
            .filter_map(|(key, _)| {
                key.strip_prefix("snapshots/")
                    .and_then(|r| r.split('/').next())
                    .and_then(|s| s.parse::<u16>().ok())
                    .map(ShardId)
            })
            .collect();
        let loaded: Vec<LoadedSnapshot> = futures::stream::iter(snapshotted)
            .map(|shard| {
                let store = store.clone();
                async move {
                    Ok(snapshot::load_latest::<ShardSnapshot>(&store, shard)
                        .await?
                        .map(|(lsn, snap)| (shard, lsn, snap)))
                }
            })
            .buffer_unordered(64)
            .collect()
            .await;
        let mut snapshot_count = 0;
        for r in loaded {
            if let Some((shard, lsn, snap)) = r? {
                shards[shard.0 as usize] = ShardState::from_snapshot(snap, lsn);
                snapshot_count += 1;
            }
        }

        // 2. Replay every segment in order, applying only records past each shard's
        //    snapshot sequence.
        let mut replayed = 0u64;
        let mut skipped = 0u64;
        let mut last_lsn: Option<Lsn> = None;
        let segments = reader::list_segments(&store, &node_id, None).await?;
        for (lsn, key) in segments {
            for mut env in reader::read_segment(&store, &key).await? {
                let st = &mut shards[env.shard.0 as usize];
                if env.shard_seq <= st.shard_seq {
                    skipped += 1;
                    continue;
                }
                if st.dirty_since_lsn.is_none() {
                    st.dirty_since_lsn = Some(lsn);
                }
                st.apply(&mut env);
                replayed += 1;
            }
            last_lsn = Some(lsn);
        }
        info!(
            node = %node_id,
            epoch,
            snapshots = snapshot_count,
            replayed,
            skipped,
            last_lsn = ?last_lsn,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "recovery complete"
        );

        // 3. Writer starts a fresh epoch.
        let check: Arc<dyn OwnershipCheck> = match &ownership {
            Some(o) => o.clone(),
            None => Arc::new(valka_wal::SingleNodeOwnership),
        };
        let writer = WalWriter::start(
            store.clone(),
            node_id.clone(),
            Lsn::new(epoch, 1),
            WalWriterConfig::from_core(&cfg.wal),
            check,
        );

        let (events, _) = broadcast::channel(4096);
        let (shutdown, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            shards: shards.into_iter().map(Mutex::new).collect(),
            writer,
            store,
            ownership,
            cfg,
            clock,
            timers: Mutex::new(TimerWheel::default()),
            pending: Mutex::new(BTreeMap::new()),
            lease_dirty: Mutex::new(HashMap::new()),
            events,
            sink: RwLock::new(sink),
            pending_wake: Notify::new(),
            shutdown,
        });
        let engine = Engine { inner };
        engine.rebuild_indexes();
        engine.spawn_loops();
        Ok(engine)
    }

    /// Replace the task sink (matching service) after construction.
    pub fn set_sink(&self, sink: Arc<dyn TaskSink>) {
        *self.inner.sink.write() = sink;
        self.inner.pending_wake.notify_one();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    pub fn node_id(&self) -> &str {
        &self.inner.cfg.node_id
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.inner.clock
    }

    pub fn durable_lsn(&self) -> Lsn {
        self.inner.writer.durable_lsn()
    }

    /// The WAL writer could not commit; the node must restart to reconverge.
    pub fn poisoned(&self) -> Option<String> {
        self.inner.writer.poisoned()
    }

    pub fn owns(&self, shard: ShardId) -> bool {
        match &self.inner.ownership {
            Some(o) => o.owns(shard),
            None => true,
        }
    }

    // ───────────────────────── commands ─────────────────────────

    pub async fn create_task(&self, req: CreateTask) -> Result<TaskView, ServerError> {
        if req.queue_name.is_empty() || req.task_name.is_empty() {
            return Err(ServerError::InvalidArgument(
                "queue_name and task_name are required".into(),
            ));
        }
        let (uuid, shard) =
            valka_core::new_task_uuid(&req.queue_name, req.idempotency_key.as_deref());
        let id = uuid.to_string();
        let now = self.inner.clock.now();
        let spec = TaskSpec {
            id: id.clone(),
            queue_name: req.queue_name.clone(),
            task_name: req.task_name,
            input: req.input,
            priority: req.priority,
            max_retries: req.max_retries,
            timeout_seconds: req.timeout_seconds,
            idempotency_key: req.idempotency_key.clone(),
            metadata: req.metadata,
            scheduled_at: req.scheduled_at,
            created_at: now,
        };
        let (durable, transitions, _, view) = self.mutate(
            shard,
            |st| {
                if let Some(k) = &req.idempotency_key
                    && st.idempotency.contains_key(k)
                {
                    return Err(ServerError::IdempotencyConflict(k.clone()));
                }
                Ok((WalRecord::TaskCreated { task: spec.clone() }, ()))
            },
            |st| st.tasks.get(&id).map(|t| t.view()),
        )?;
        durable.wait().await?;
        valka_core::metrics::record_task_created(&req.queue_name);
        self.after_durable(&transitions);
        view.ok_or_else(|| ServerError::Internal("task vanished after create".into()))
    }

    pub fn get_task(&self, task_id: &str) -> Option<TaskView> {
        let shard = shard_of_task_id(task_id)?;
        self.inner.shards[shard.0 as usize]
            .lock()
            .tasks
            .get(task_id)
            .map(|t| t.view())
    }

    pub fn list_tasks(
        &self,
        queue: Option<&str>,
        status: Option<TaskStatus>,
        limit: usize,
        offset: usize,
    ) -> Vec<TaskView> {
        let mut all: Vec<TaskView> = Vec::new();
        for m in &self.inner.shards {
            let st = m.lock();
            for t in st.tasks.values() {
                if queue.is_some_and(|q| q != t.spec.queue_name) {
                    continue;
                }
                if status.is_some_and(|s| s != t.status) {
                    continue;
                }
                all.push(t.view());
            }
        }
        all.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        all.into_iter().skip(offset).take(limit).collect()
    }

    pub fn count_tasks(&self, queue: Option<&str>, status: Option<TaskStatus>) -> usize {
        self.inner
            .shards
            .iter()
            .map(|m| {
                m.lock()
                    .tasks
                    .values()
                    .filter(|t| {
                        queue.is_none_or(|q| q == t.spec.queue_name)
                            && !status.is_some_and(|s| s != t.status)
                    })
                    .count()
            })
            .sum()
    }

    /// Cancel a non-terminal task. Returns the view and the worker to notify if running.
    pub async fn cancel_task(
        &self,
        task_id: &str,
        reason: &str,
    ) -> Result<(TaskView, Option<String>), ServerError> {
        let shard = self.shard_for(task_id)?;
        let (durable, transitions, _, view) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                if t.is_terminal() {
                    return Err(ServerError::InvalidStatusTransition {
                        from: t.status.as_str().into(),
                        to: "CANCELLED".into(),
                    });
                }
                Ok((
                    WalRecord::TaskCancelled {
                        task_id: task_id.into(),
                        reason: reason.into(),
                    },
                    (),
                ))
            },
            |st| st.tasks.get(task_id).map(|t| t.view()),
        )?;
        durable.wait().await?;
        self.after_durable(&transitions);
        let worker = transitions.first().and_then(|t| t.worker_id.clone());
        let view =
            view.ok_or_else(|| ServerError::Internal("task vanished after cancel".into()))?;
        Ok((view, worker))
    }

    pub async fn delete_task(&self, task_id: &str) -> Result<bool, ServerError> {
        let Some(shard) = shard_of_task_id(task_id) else {
            return Ok(false);
        };
        let res = self.mutate(
            shard,
            |st| {
                if !st.tasks.contains_key(task_id) {
                    return Err(ServerError::TaskNotFound(task_id.into()));
                }
                Ok((
                    WalRecord::TaskDeleted {
                        task_id: task_id.into(),
                    },
                    (),
                ))
            },
            |_| (),
        );
        match res {
            Ok((durable, transitions, _, _)) => {
                durable.wait().await?;
                self.after_durable(&transitions);
                Ok(true)
            }
            Err(ServerError::TaskNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Drop every task in every owned shard. Returns the count removed.
    pub async fn clear_all_tasks(&self) -> Result<usize, ServerError> {
        let mut total = 0usize;
        let mut waits = Vec::new();
        let mut all_transitions = Vec::new();
        for shard in ShardId::all() {
            let res = self.mutate(
                shard,
                |st| {
                    if st.is_empty() {
                        return Err(ServerError::Internal("empty".into()));
                    }
                    Ok((WalRecord::ShardCleared, st.tasks.len()))
                },
                |_| (),
            );
            if let Ok((d, tr, n, _)) = res {
                total += n;
                waits.push(d);
                all_transitions.extend(tr);
            }
        }
        for d in waits {
            d.wait().await?;
        }
        self.after_durable(&all_transitions);
        Ok(total)
    }

    /// Record a dispatch. Does **not** wait for durability: the dispatch record is
    /// asynchronous by design (see DESIGN.md §5); a lost dispatch record only costs a
    /// possible duplicate execution, which the at-least-once contract already allows.
    pub fn dispatch(&self, task_id: &str, worker_id: &str) -> Result<DispatchInfo, ServerError> {
        let shard = self.shard_for(task_id)?;
        let now = self.inner.clock.now();
        let run_id = valka_core::shard::embed_shard(uuid::Uuid::now_v7(), shard).to_string();
        let node_id = self.inner.cfg.node_id.clone();
        let grace = self.inner.cfg.scheduler.lease_grace_secs;
        let (durable, transitions, info, _) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                if t.status != TaskStatus::Pending || !t.due {
                    return Err(ServerError::InvalidStatusTransition {
                        from: t.status.as_str().into(),
                        to: "RUNNING".into(),
                    });
                }
                let attempt = t.attempt_count + 1;
                let lease_until =
                    now + ChronoDuration::seconds(t.spec.timeout_seconds as i64 + grace);
                let info = DispatchInfo {
                    run_id: run_id.clone(),
                    attempt,
                    lease_until,
                    task: dispatchable(t, attempt),
                };
                Ok((
                    WalRecord::TaskDispatched {
                        task_id: task_id.into(),
                        run_id: run_id.clone(),
                        attempt,
                        worker_id: worker_id.into(),
                        node_id: node_id.clone(),
                        lease_until,
                    },
                    info,
                ))
            },
            |_| (),
        )?;
        self.inner.timers.lock().schedule(
            info.lease_until,
            TimerKind::LeaseExpiry {
                task_id: task_id.into(),
                run_id: info.run_id.clone(),
            },
        );
        self.spawn_after_durable(durable, transitions);
        Ok(info)
    }

    /// The matching layer dropped a task it had been offered; make it runnable again.
    pub fn unoffer(&self, task_id: &str) {
        let Some(shard) = shard_of_task_id(task_id) else {
            return;
        };
        let key = {
            let mut st = self.inner.shards[shard.0 as usize].lock();
            let Some(t) = st.tasks.get_mut(task_id) else {
                return;
            };
            t.offered = false;
            if t.is_runnable() {
                Some((t.spec.queue_name.clone(), pending_key(t)))
            } else {
                None
            }
        };
        if let Some((q, k)) = key {
            self.inner.pending.lock().entry(q).or_default().insert(k);
            self.inner.pending_wake.notify_one();
        }
    }

    pub async fn complete_run(
        &self,
        task_id: &str,
        run_id: &str,
        output: Option<Value>,
    ) -> Result<TaskView, ServerError> {
        let shard = self.shard_for(task_id)?;
        let (durable, transitions, _, view) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                ensure_running(t, run_id)?;
                Ok((
                    WalRecord::RunCompleted {
                        task_id: task_id.into(),
                        run_id: run_id.into(),
                        output: output.clone(),
                    },
                    (),
                ))
            },
            |st| st.tasks.get(task_id).map(|t| t.view()),
        )?;
        durable.wait().await?;
        self.after_durable(&transitions);
        view.ok_or_else(|| ServerError::Internal("task vanished after completion".into()))
    }

    pub async fn fail_run(
        &self,
        task_id: &str,
        run_id: &str,
        error: &str,
        retryable: bool,
    ) -> Result<FailResult, ServerError> {
        let shard = self.shard_for(task_id)?;
        let now = self.inner.clock.now();
        let sched = self.inner.cfg.scheduler.clone();
        let (durable, transitions, outcome, _) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                ensure_running(t, run_id)?;
                let outcome =
                    decide_outcome(t.attempt_count, t.spec.max_retries, retryable, now, &sched);
                Ok((
                    WalRecord::RunFailed {
                        task_id: task_id.into(),
                        run_id: run_id.into(),
                        error: error.into(),
                        outcome: outcome.clone(),
                    },
                    outcome,
                ))
            },
            |_| (),
        )?;
        self.arm_timers_for(&transitions);
        durable.wait().await?;
        self.after_durable(&transitions);
        Ok(FailResult { outcome })
    }

    /// Extend leases in RAM now; the WAL record is coalesced by the ticker.
    pub fn heartbeat(&self, task_ids: &[String]) {
        let now = self.inner.clock.now();
        let lease = now + ChronoDuration::seconds(self.inner.cfg.scheduler.heartbeat_lease_secs);
        for task_id in task_ids {
            let Some(shard) = shard_of_task_id(task_id) else {
                continue;
            };
            let mut st = self.inner.shards[shard.0 as usize].lock();
            let Some(t) = st.tasks.get_mut(task_id) else {
                continue;
            };
            if t.status != TaskStatus::Running {
                continue;
            }
            let Some(run) = t.current_run_mut() else {
                continue;
            };
            if run.status != RunStatus::Running {
                continue;
            }
            run.lease_until = run.lease_until.max(lease);
            run.last_heartbeat = now;
            let run_id = run.id.clone();
            drop(st);
            self.inner
                .lease_dirty
                .lock()
                .insert(task_id.clone(), (shard, run_id, lease));
        }
    }

    pub fn runs_for_task(&self, task_id: &str) -> Option<Vec<RunView>> {
        let shard = shard_of_task_id(task_id)?;
        self.inner.shards[shard.0 as usize]
            .lock()
            .tasks
            .get(task_id)
            .map(|t| t.run_views())
    }

    pub fn list_dead_letters(
        &self,
        queue: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Vec<DeadLetterView> {
        let mut all: Vec<DeadLetterView> = Vec::new();
        for m in &self.inner.shards {
            let st = m.lock();
            all.extend(
                st.dead_letters
                    .values()
                    .filter(|d| queue.is_none_or(|q| q == d.queue_name))
                    .map(|d| d.view()),
            );
        }
        all.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        all.into_iter().skip(offset).take(limit).collect()
    }

    // ───────────────────────── signals ─────────────────────────

    pub async fn send_signal(
        &self,
        task_id: &str,
        name: &str,
        payload: Option<Value>,
    ) -> Result<SignalView, ServerError> {
        let shard = self.shard_for(task_id)?;
        let signal_id = valka_core::shard::embed_shard(uuid::Uuid::now_v7(), shard).to_string();
        let (durable, _, _, view) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                if t.is_terminal() {
                    return Err(ServerError::InvalidStatusTransition {
                        from: t.status.as_str().into(),
                        to: "SIGNAL".into(),
                    });
                }
                Ok((
                    WalRecord::SignalCreated {
                        signal_id: signal_id.clone(),
                        task_id: task_id.into(),
                        signal_name: name.into(),
                        payload: payload.clone(),
                    },
                    (),
                ))
            },
            |st| st.signals.get(&signal_id).map(|s| s.view()),
        )?;
        durable.wait().await?;
        view.ok_or_else(|| ServerError::Internal("signal vanished".into()))
    }

    pub fn signal_delivered(&self, signal_id: &str) {
        self.signal_transition(
            signal_id,
            |task_id| WalRecord::SignalDelivered {
                signal_id: signal_id.into(),
                task_id,
            },
            SignalStatus::Pending,
        );
    }

    pub fn signal_acked(&self, signal_id: &str) {
        self.signal_transition(
            signal_id,
            |task_id| WalRecord::SignalAcked {
                signal_id: signal_id.into(),
                task_id,
            },
            SignalStatus::Delivered,
        );
    }

    fn signal_transition(
        &self,
        signal_id: &str,
        make: impl Fn(String) -> WalRecord,
        expect: SignalStatus,
    ) {
        let Some(shard) = shard_of_task_id(signal_id) else {
            return;
        };
        let res = self.mutate(
            shard,
            |st| {
                let s = st
                    .signals
                    .get(signal_id)
                    .ok_or_else(|| ServerError::TaskNotFound(signal_id.into()))?;
                if s.status != expect {
                    return Err(ServerError::InvalidStatusTransition {
                        from: s.status.as_str().into(),
                        to: "".into(),
                    });
                }
                Ok((make(s.task_id.clone()), ()))
            },
            |_| (),
        );
        if let Ok((d, tr, _, _)) = res {
            self.spawn_after_durable(d, tr);
        }
    }

    /// Worker disconnected: delivered-but-unacked signals go back to PENDING.
    pub fn reset_signals(&self, task_id: &str) {
        let Some(shard) = shard_of_task_id(task_id) else {
            return;
        };
        let res = self.mutate(
            shard,
            |st| {
                let any = st
                    .signals
                    .values()
                    .any(|s| s.task_id == task_id && s.status == SignalStatus::Delivered);
                if !any {
                    return Err(ServerError::Internal("nothing to reset".into()));
                }
                Ok((
                    WalRecord::SignalsReset {
                        task_id: task_id.into(),
                    },
                    (),
                ))
            },
            |_| (),
        );
        if let Ok((d, tr, _, _)) = res {
            self.spawn_after_durable(d, tr);
        }
    }

    pub fn list_signals(&self, task_id: &str, status: Option<SignalStatus>) -> Vec<SignalView> {
        let Some(shard) = shard_of_task_id(task_id) else {
            return Vec::new();
        };
        let st = self.inner.shards[shard.0 as usize].lock();
        let mut v: Vec<SignalView> = st
            .signals
            .values()
            .filter(|s| s.task_id == task_id && !status.is_some_and(|x| x != s.status))
            .map(|s| s.view())
            .collect();
        v.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        v
    }

    pub fn pending_signals(&self, task_id: &str) -> Vec<SignalView> {
        self.list_signals(task_id, Some(SignalStatus::Pending))
    }

    // ───────────────────────── pending / feeder ─────────────────────────

    pub fn queues(&self) -> Vec<String> {
        let mut qs: BTreeSet<String> = BTreeSet::new();
        for m in &self.inner.shards {
            for t in m.lock().tasks.values() {
                qs.insert(t.spec.queue_name.clone());
            }
        }
        qs.into_iter().collect()
    }

    pub fn pending_count(&self, queue: &str) -> usize {
        self.inner.pending.lock().get(queue).map_or(0, |s| s.len())
    }

    /// Pull up to `max` runnable tasks for a queue, marking them offered.
    pub fn take_pending(&self, queue: &str, max: usize) -> Vec<DispatchableTask> {
        if max == 0 {
            return Vec::new();
        }
        let keys: Vec<PendingKey> = {
            let mut p = self.inner.pending.lock();
            let Some(set) = p.get_mut(queue) else {
                return Vec::new();
            };
            let mut out = Vec::with_capacity(max.min(set.len()));
            while out.len() < max {
                let Some(k) = set.pop_first() else { break };
                out.push(k);
            }
            if set.is_empty() {
                p.remove(queue);
            }
            out
        };
        let mut out = Vec::with_capacity(keys.len());
        for (_, _, task_id) in keys {
            let Some(shard) = shard_of_task_id(&task_id) else {
                continue;
            };
            let mut st = self.inner.shards[shard.0 as usize].lock();
            let Some(t) = st.tasks.get_mut(&task_id) else {
                continue;
            };
            if !t.is_runnable() {
                continue;
            }
            t.offered = true;
            out.push(dispatchable(t, t.attempt_count + 1));
        }
        out
    }

    // ───────────────────────── lifecycle ─────────────────────────

    /// Flush pending records, snapshot dirty shards, stop loops.
    pub async fn shutdown(&self) -> Result<(), ServerError> {
        let _ = self.inner.shutdown.send(true);
        self.flush_lease_records();
        self.inner.writer.sync().await?;
        self.snapshot_dirty_shards(true).await;
        Ok(())
    }

    /// Force a snapshot round now (tests).
    pub async fn snapshot_now(&self) {
        self.inner.writer.sync().await.ok();
        self.snapshot_dirty_shards(true).await;
    }

    /// Wait until everything appended so far is durable.
    pub async fn sync(&self) -> Result<Lsn, ServerError> {
        Ok(self.inner.writer.sync().await?)
    }

    // ───────────────────────── internals ─────────────────────────

    fn shard_for(&self, task_id: &str) -> Result<ShardId, ServerError> {
        let shard =
            shard_of_task_id(task_id).ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
        if !self.owns(shard) {
            return Err(ServerError::NotOwner(shard.0));
        }
        Ok(shard)
    }

    /// Validate + build a record under the shard lock, apply it, append it. Returns the
    /// durability future, the transitions, and whatever `after` reads from the new state.
    pub(crate) fn mutate<R, V>(
        &self,
        shard: ShardId,
        build: impl FnOnce(&ShardState) -> Result<(WalRecord, R), ServerError>,
        after: impl FnOnce(&ShardState) -> V,
    ) -> Result<(Durable, Vec<Transition>, R, V), ServerError> {
        let mut st = self.inner.shards[shard.0 as usize].lock();
        let (record, ret) = build(&st)?;
        let mut env = Envelope::with_ts(shard, self.inner.clock.now(), record);
        let was_clean = st.records_since_snapshot == 0;
        let transitions = st.apply(&mut env);
        if was_clean {
            st.dirty_since_lsn = Some(self.inner.writer.next_lsn());
        }
        let view = after(&st);
        let durable = self.inner.writer.append(vec![env]);
        drop(st);
        // Tasks that left PENDING must leave the runnable index right away.
        self.drop_from_pending(&transitions);
        Ok((durable, transitions, ret, view))
    }

    pub(crate) fn drop_from_pending(&self, transitions: &[Transition]) {
        let leaving: Vec<&Transition> = transitions
            .iter()
            .filter(|t| t.from == Some(TaskStatus::Pending) && t.to != Some(TaskStatus::Pending))
            .collect();
        if leaving.is_empty() {
            return;
        }
        let mut p = self.inner.pending.lock();
        for t in leaving {
            if let Some(set) = p.get_mut(&t.queue_name) {
                set.retain(|(_, _, id)| *id != t.task_id);
                if set.is_empty() {
                    p.remove(&t.queue_name);
                }
            }
        }
    }

    pub(crate) fn arm_timers_for(&self, transitions: &[Transition]) {
        for tr in transitions {
            if tr.to == Some(TaskStatus::Retry) {
                let Some(shard) = shard_of_task_id(&tr.task_id) else {
                    continue;
                };
                let at = self.inner.shards[shard.0 as usize]
                    .lock()
                    .tasks
                    .get(&tr.task_id)
                    .and_then(|t| t.next_attempt_at);
                if let Some(at) = at {
                    self.inner.timers.lock().schedule(
                        at,
                        TimerKind::Promote {
                            task_id: tr.task_id.clone(),
                        },
                    );
                }
            }
        }
    }

    pub(crate) fn spawn_after_durable(&self, durable: Durable, transitions: Vec<Transition>) {
        let me = self.clone();
        tokio::spawn(async move {
            match durable.wait().await {
                Ok(_) => me.after_durable(&transitions),
                Err(e) => warn!(error = %e, "record not durable; transitions not published"),
            }
        });
    }

    /// Runs once records are durable: publish events, index newly runnable tasks, offer
    /// them to the sink, arm timers.
    pub(crate) fn after_durable(&self, transitions: &[Transition]) {
        let now = self.inner.clock.now();
        for tr in transitions {
            let _ = self.inner.events.send(EngineEvent {
                task_id: tr.task_id.clone(),
                queue_name: tr.queue_name.clone(),
                previous: tr.from,
                new: tr.to,
                worker_id: tr.worker_id.clone(),
                node_id: self.inner.cfg.node_id.clone(),
                attempt: tr.attempt,
                error: tr.error.clone(),
                ts: now,
            });
            match tr.to {
                Some(TaskStatus::Completed) => {
                    valka_core::metrics::record_task_completed(&tr.queue_name)
                }
                Some(TaskStatus::Failed) => valka_core::metrics::record_task_failed(&tr.queue_name),
                Some(TaskStatus::Retry) => valka_core::metrics::record_task_retried(&tr.queue_name),
                Some(TaskStatus::DeadLetter) => {
                    valka_core::metrics::record_task_dead_lettered(&tr.queue_name)
                }
                _ => {}
            }
            if tr.to == Some(TaskStatus::Pending) {
                self.index_or_offer(&tr.task_id);
            }
        }
        self.arm_timers_for(transitions);
    }

    /// A task just became PENDING: if due, try the sink (hot path); otherwise arm its
    /// promote timer.
    pub(crate) fn index_or_offer(&self, task_id: &str) {
        let Some(shard) = shard_of_task_id(task_id) else {
            return;
        };
        let (dispatchable, promote_at, key, queue) = {
            let mut st = self.inner.shards[shard.0 as usize].lock();
            let Some(t) = st.tasks.get_mut(task_id) else {
                return;
            };
            if t.status != TaskStatus::Pending {
                return;
            }
            if !t.due {
                (None, t.next_attempt_at, None, t.spec.queue_name.clone())
            } else if t.offered {
                return;
            } else {
                t.offered = true;
                (
                    Some(dispatchable(t, t.attempt_count + 1)),
                    None,
                    Some(pending_key(t)),
                    t.spec.queue_name.clone(),
                )
            }
        };
        if let Some(at) = promote_at {
            self.inner.timers.lock().schedule(
                at,
                TimerKind::Promote {
                    task_id: task_id.into(),
                },
            );
            return;
        }
        let Some(d) = dispatchable else { return };
        let sink = self.inner.sink.read().clone();
        match sink.offer(d) {
            OfferOutcome::Matched | OfferOutcome::Buffered => {}
            OfferOutcome::Rejected => {
                if let Some(t) = self.inner.shards[shard.0 as usize]
                    .lock()
                    .tasks
                    .get_mut(task_id)
                {
                    t.offered = false;
                }
                if let Some(k) = key {
                    self.inner
                        .pending
                        .lock()
                        .entry(queue)
                        .or_default()
                        .insert(k);
                    self.inner.pending_wake.notify_one();
                }
            }
        }
    }
}

// ───────────────────────── helpers ─────────────────────────

pub(crate) fn pending_key(t: &crate::state::TaskState) -> PendingKey {
    (
        -t.spec.priority,
        t.spec.created_at.timestamp_millis(),
        t.spec.id.clone(),
    )
}

pub(crate) fn dispatchable(t: &crate::state::TaskState, attempt: i32) -> DispatchableTask {
    DispatchableTask {
        task_id: t.spec.id.clone(),
        queue_name: t.spec.queue_name.clone(),
        task_name: t.spec.task_name.clone(),
        input: t.spec.input.clone(),
        attempt_number: attempt,
        timeout_seconds: t.spec.timeout_seconds,
        metadata: t.spec.metadata.clone(),
        priority: t.spec.priority,
    }
}

fn ensure_running(t: &crate::state::TaskState, run_id: &str) -> Result<(), ServerError> {
    if t.status != TaskStatus::Running {
        return Err(ServerError::InvalidStatusTransition {
            from: t.status.as_str().into(),
            to: "FINISHED".into(),
        });
    }
    match t.runs.iter().find(|r| r.id == run_id) {
        Some(r) if r.status == RunStatus::Running => Ok(()),
        Some(_) => Err(ServerError::InvalidStatusTransition {
            from: "FINISHED_RUN".into(),
            to: "FINISHED".into(),
        }),
        None => Err(ServerError::LeaseExpired(format!("run {run_id} not found"))),
    }
}

pub(crate) fn decide_outcome(
    attempt: i32,
    max_retries: i32,
    retryable: bool,
    now: DateTime<Utc>,
    sched: &SchedulerConfig,
) -> FailureOutcome {
    if retryable && attempt < max_retries {
        FailureOutcome::Retry {
            at: now
                + compute_retry_delay(
                    attempt,
                    sched.retry_base_delay_secs,
                    sched.retry_max_delay_secs,
                ),
        }
    } else if attempt >= max_retries {
        FailureOutcome::DeadLetter
    } else {
        FailureOutcome::Failed
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}
