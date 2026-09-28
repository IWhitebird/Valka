//! The engine: command API, recovery, and background loops.

use chrono::{DateTime, Utc};
use futures::StreamExt;
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tokio::sync::{Notify, broadcast, watch};
use tracing::info;
use valka_core::{SchedulerConfig, ServerError, ShardId, TaskStatus, WalConfig};
use valka_wal::lsn::keys;
use valka_wal::ownership::{Ownership, OwnershipCheck};
use valka_wal::{FailureOutcome, Lsn, Store, WalWriter, WalWriterConfig, reader, snapshot};

use crate::clock::{Clock, TokioClock};
use crate::sink::{DispatchableTask, NoopSink, TaskSink};
use crate::state::{ShardSnapshot, ShardState};
use crate::timers::TimerWheel;
use crate::view::CheckpointView;

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
    pub checkpoints: Vec<CheckpointView>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FailResult {
    pub outcome: FailureOutcome,
}

/// What a worker reports for a run.
#[derive(Debug, Clone, PartialEq)]
pub enum RunResult {
    Completed(Option<Value>),
    Failed { error: String, retryable: bool },
}

/// The answer to a reported result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultOutcome {
    /// Durable: recorded now, or by an earlier delivery of the same result.
    Applied,
    /// The run already ended another way; the result was not recorded.
    Stale,
    /// Not recorded; report it again later.
    Retry,
}

/// Runnable tasks ordered by priority desc, then creation order.
pub(crate) type PendingKey = (i32, i64, String);
/// task_id -> (shard, run_id, lease_until) awaiting a coalesced `LeaseExtended` record.
pub(crate) type LeaseDirty = HashMap<String, (ShardId, String, DateTime<Utc>)>;
pub(crate) type LoadedSnapshot = Result<Option<(ShardId, Lsn, ShardSnapshot)>, ServerError>;

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
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) last_snapshot_round: Mutex<Option<DateTime<Utc>>>,
    /// Writer bytes committed when the last full snapshot round started.
    pub(crate) log_mark: AtomicU64,
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
        let clock_now = clock.now();
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
            started_at: clock_now,
            last_snapshot_round: Mutex::new(None),
            log_mark: AtomicU64::new(0),
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

    pub fn poison_watch(&self) -> watch::Receiver<Option<String>> {
        self.inner.writer.poison_watch()
    }

    pub fn owns(&self, shard: ShardId) -> bool {
        match &self.inner.ownership {
            Some(o) => o.owns(shard),
            None => true,
        }
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
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}
