//! The engine: command API, node start, and background loops.

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tokio::sync::{Notify, broadcast, watch};
use tracing::{info, warn};
use valka_core::{ClusterConfig, SchedulerConfig, ServerError, ShardId, TaskStatus, WalConfig};
use valka_wal::lease::{LeaseConfig, LeaseKeeper, NodeAdvert};
use valka_wal::{ClusterMeta, FailureOutcome, Lsn, Store, WalWriter, WalWriterConfig, meta};

use crate::clock::{Clock, TokioClock};
use crate::cluster::Membership;
use crate::sink::{DispatchableTask, NoopSink, TaskSink};
use crate::state::{ShardPhase, ShardState};
use crate::timers::TimerWheel;
use crate::view::CheckpointView;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub node_id: String,
    pub wal: WalConfig,
    pub scheduler: SchedulerConfig,
    pub cluster: ClusterConfig,
    /// Addresses advertised in this node's lease.
    pub advert: NodeAdvert,
    /// Feeder cadence (from `MatchingConfig`).
    pub feeder_interval: Duration,
    pub feeder_batch_size: usize,
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
            cluster: ClusterConfig {
                num_shards: 256,
                ..ClusterConfig::for_tests()
            },
            advert: NodeAdvert {
                version: "test".into(),
                ..Default::default()
            },
            feeder_interval: Duration::from_millis(10),
            feeder_batch_size: 100,
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DispatchInfo {
    pub run_id: String,
    pub attempt: i32,
    pub lease_until: DateTime<Utc>,
    pub task: DispatchableTask,
    pub checkpoints: Vec<CheckpointView>,
    /// Signals created before the run started, to deliver right after the assignment.
    #[serde(default)]
    pub pending_signals: Vec<crate::view::SignalView>,
}

/// The owner's answer to a worker that reconnects with a run in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReattachOutcome {
    /// The run is current; it now lives on the worker's new node.
    Reattached,
    /// Its dispatch record was lost in a crash; the worker's run was recorded now.
    Adopted,
    /// The run ended or was superseded: the worker must stop it.
    Stale,
}

/// Where a running run lives: which worker, attached to which node.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunLocation {
    pub run_id: String,
    pub worker_id: String,
    pub node_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FailResult {
    pub outcome: FailureOutcome,
}

/// What a worker reports for a run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RunResult {
    Completed(Option<Value>),
    Failed { error: String, retryable: bool },
}

/// The answer to a reported result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

pub(crate) struct Inner {
    pub(crate) shards: Vec<Mutex<ShardState>>,
    pub(crate) num_shards: u32,
    pub(crate) meta: ClusterMeta,
    pub(crate) writer: WalWriter,
    pub(crate) lease: Arc<LeaseKeeper>,
    pub(crate) store: Store,
    pub(crate) cfg: EngineConfig,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) timers: Mutex<TimerWheel>,
    pub(crate) pending: Mutex<BTreeMap<String, BTreeSet<PendingKey>>>,
    pub(crate) lease_dirty: Mutex<LeaseDirty>,
    pub(crate) events: broadcast::Sender<EngineEvent>,
    pub(crate) sink: RwLock<Arc<dyn TaskSink>>,
    pub(crate) pending_wake: Notify,
    pub(crate) shutdown: watch::Sender<bool>,
    /// Stops lease renewal. Separate from `shutdown`: a drain or shutdown keeps renewing
    /// while it hands shards over, and stops only once the lease is marked dead.
    pub(crate) lease_stop: watch::Sender<bool>,
    /// Set once when this node must exit: the WAL writer failed or the lease was lost.
    pub(crate) poison: watch::Sender<Option<String>>,
    /// Set once a drain finished: the process should exit (cleanly).
    pub(crate) drained: watch::Sender<bool>,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) last_snapshot_round: Mutex<Option<DateTime<Utc>>>,
    /// Writer bytes committed when the last full snapshot round started.
    pub(crate) log_mark: AtomicU64,
    pub(crate) membership: Membership,
    /// Serialises claim and release rounds.
    pub(crate) moves: tokio::sync::Mutex<()>,
    /// When each shard last moved to or from this node (for `min_residency`).
    pub(crate) moved_at: Mutex<HashMap<ShardId, tokio::time::Instant>>,
    /// Shards whose owner record this incarnation took (claim CAS) but which it does not
    /// serve because a later claim step failed. Nobody else can claim them while this
    /// incarnation lives, so this node retries them whatever placement says (and hands
    /// them over normally once it serves them).
    pub(crate) stranded: Mutex<std::collections::HashSet<ShardId>>,
    /// PENDING tasks recovered by a claim are not dispatched before this (reconcile grace).
    pub(crate) hold_until: Mutex<HashMap<ShardId, tokio::time::Instant>>,
    /// Offered tasks (local matching or another node's `fetch`) not dispatched yet, with the
    /// time their offer lapses.
    pub(crate) offers: Mutex<HashMap<String, tokio::time::Instant>>,
    /// Woken whenever runnable tasks are added to the pending index (remote fetchers).
    pub(crate) runnable: Notify,
    /// Queue configs (partition counts), cached for `QUEUE_CONFIG_TTL`.
    pub(crate) queues:
        Mutex<HashMap<String, (valka_wal::queues::QueueConfig, tokio::time::Instant)>>,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

impl Engine {
    /// Start a node on a bucket: take the lease (fencing the previous run of this node id),
    /// split the logs it left behind, claim the shards placement gives it, and start the
    /// background loops.
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
        if !valka_wal::valid_node_id(&node_id) {
            return Err(ServerError::InvalidArgument(format!(
                "node id `{node_id}`: use 1-64 characters from [A-Za-z0-9_-]"
            )));
        }
        let meta = meta::ensure(&store, cfg.cluster.num_shards).await?;
        if cfg.cluster.enabled {
            if matches!(store.backend_label(), "local" | "memory") {
                return Err(ServerError::InvalidArgument(format!(
                    "cluster.enabled needs an S3-compatible bucket; the `{}` backend only \
                     works inside one process",
                    store.backend_label()
                )));
            }
            if cfg.cluster.token.as_deref().is_none_or(str::is_empty) {
                return Err(ServerError::InvalidArgument(
                    "cluster.enabled requires cluster.token".into(),
                ));
            }
            meta::self_test(&store, &node_id).await?;
        }

        // A node must not join a cluster that already runs a newer protocol: it could
        // misread what the newer nodes write.
        for (node, _) in valka_wal::lease::list(&store).await? {
            if let Some(l) = valka_wal::lease::get(&store, &node).await?
                && l.record.state != valka_wal::LeaseState::Dead
                && l.record.protocol > valka_wal::lease::PROTOCOL
                && node != node_id
            {
                return Err(ServerError::InvalidArgument(format!(
                    "node {node} runs protocol {} but this build speaks {}; upgrade this node",
                    l.record.protocol,
                    valka_wal::lease::PROTOCOL
                )));
            }
        }

        let (lease, _prev) = LeaseKeeper::start(
            store.clone(),
            &node_id,
            cfg.advert.clone(),
            LeaseConfig::from_core(&cfg.cluster),
        )
        .await?;
        let inc = lease.inc();
        let writer = WalWriter::start(
            store.clone(),
            node_id.clone(),
            Lsn::new(inc, 1),
            WalWriterConfig::from_core(&cfg.wal),
            lease.clone(),
        );

        let num_shards = meta.num_shards;
        let (events, _) = broadcast::channel(4096);
        let (shutdown, _) = watch::channel(false);
        let clock_now = clock.now();
        let membership = Membership::new(&node_id, &cfg.cluster);
        let inner = Arc::new(Inner {
            shards: ShardId::all(num_shards)
                .map(|s| Mutex::new(ShardState::new(s)))
                .collect(),
            num_shards,
            meta,
            writer,
            lease,
            store,
            cfg,
            clock,
            timers: Mutex::new(TimerWheel::default()),
            pending: Mutex::new(BTreeMap::new()),
            lease_dirty: Mutex::new(HashMap::new()),
            events,
            sink: RwLock::new(sink),
            pending_wake: Notify::new(),
            shutdown,
            lease_stop: watch::Sender::new(false),
            poison: watch::Sender::new(None),
            drained: watch::Sender::new(false),
            started_at: clock_now,
            last_snapshot_round: Mutex::new(None),
            log_mark: AtomicU64::new(0),
            membership,
            moves: tokio::sync::Mutex::new(()),
            moved_at: Mutex::new(HashMap::new()),
            stranded: Mutex::new(std::collections::HashSet::new()),
            hold_until: Mutex::new(HashMap::new()),
            queues: Mutex::new(HashMap::new()),
            offers: Mutex::new(HashMap::new()),
            runnable: Notify::new(),
        });
        let engine = Engine { inner };
        engine.forward_poison();
        let started = std::time::Instant::now();

        // The logs this node id left behind: split them so their shards can be claimed.
        for u in engine.inner.lease.record().unsplit {
            engine.split_incarnation(&node_id, u.inc, u.trunc).await?;
            engine.inner.lease.drop_unsplit(u.inc).await?;
        }
        engine.spawn_loops();
        // First placement round now, so a node that can serve does before `open` returns.
        engine.refresh_membership().await;
        let claimed = engine.placement_round().await;
        info!(
            node = %node_id,
            inc,
            num_shards,
            claimed,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "node started"
        );
        Ok(engine)
    }

    /// A local worker started waiting: let the feeder offer it pending tasks now.
    pub fn wake_feeder(&self) {
        self.inner.pending_wake.notify_one();
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

    /// This process's incarnation.
    pub fn incarnation(&self) -> u32 {
        self.inner.lease.inc()
    }

    pub fn num_shards(&self) -> u32 {
        self.inner.num_shards
    }

    pub fn cluster_meta(&self) -> &ClusterMeta {
        &self.inner.meta
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.inner.clock
    }

    pub fn config(&self) -> &EngineConfig {
        &self.inner.cfg
    }

    pub fn durable_lsn(&self) -> Lsn {
        self.inner.writer.durable_lsn()
    }

    /// Why this node must exit (WAL writer failed, or lease lost), once it must.
    pub fn poisoned(&self) -> Option<String> {
        self.inner.poison.borrow().clone()
    }

    pub fn poison_watch(&self) -> watch::Receiver<Option<String>> {
        self.inner.poison.subscribe()
    }

    /// Renewed recently enough that nobody can have taken over this node's shards.
    pub fn is_serving(&self) -> bool {
        self.inner.lease.is_serving() && self.poisoned().is_none()
    }

    /// This node currently serves `shard`.
    pub fn owns(&self, shard: ShardId) -> bool {
        self.inner
            .shards
            .get(shard.index())
            .is_some_and(|m| m.lock().serving())
    }

    pub fn shard_phase(&self, shard: ShardId) -> Option<ShardPhase> {
        self.inner.shards.get(shard.index()).map(|m| m.lock().phase)
    }

    /// Shards this node serves.
    pub fn owned_shards(&self) -> Vec<ShardId> {
        self.inner
            .shards
            .iter()
            .enumerate()
            .filter(|(_, m)| m.lock().serving())
            .map(|(i, _)| ShardId(i as u16))
            .collect()
    }

    fn forward_poison(&self) {
        let poison = self.inner.poison.clone();
        let mut writer = self.inner.writer.poison_watch();
        let mut lease = self.inner.lease.fence_watch();
        tokio::spawn(async move {
            let why = tokio::select! {
                r = writer.wait_for(|p| p.is_some()) => r.ok().and_then(|p| p.clone()),
                r = lease.wait_for(|p| p.is_some()) => r.ok().and_then(|p| p.clone()),
            };
            if let Some(why) = why {
                poison.send_replace(Some(why));
            }
        });
    }

    // ───────────────────────── lifecycle ─────────────────────────

    /// Flush pending records, snapshot dirty shards, stop loops, and mark the lease dead
    /// so nobody waits out a lease timeout. The shards stay owned by this incarnation;
    /// the next start of this node id (or any other node) splits what is left of its log
    /// and claims them. Use [`Engine::drain`] to hand them over to live peers instead.
    pub async fn shutdown(&self) -> Result<(), ServerError> {
        let _ = self.inner.shutdown.send(true);
        self.flush_lease_records();
        self.inner.writer.sync().await?;
        self.snapshot_dirty_shards(true).await;
        self.inner
            .lease
            .set_state(valka_wal::LeaseState::Dead)
            .await?;
        let _ = self.inner.lease_stop.send(true);
        Ok(())
    }

    /// Stop on SIGTERM: hand shards over when live peers can take them, otherwise keep
    /// them (a restart of this node reclaims them without replaying much).
    pub async fn stop(&self) -> Result<(), ServerError> {
        if self.live_peers().is_empty() {
            self.shutdown().await
        } else {
            self.drain().await
        }
    }

    /// Graceful exit: mark the lease draining, release every shard with a final snapshot
    /// (so whoever takes them over replays nothing), then mark the lease dead.
    pub async fn drain(&self) -> Result<(), ServerError> {
        self.inner
            .lease
            .set_state(valka_wal::LeaseState::Draining)
            .await?;
        let _ = self.inner.shutdown.send(true);
        self.flush_lease_records();
        self.inner.writer.sync().await?;
        let shards = self.owned_shards();
        let released = self.release_shards(shards.clone(), usize::MAX).await;
        if released < shards.len() {
            warn!(
                released,
                owned = shards.len(),
                "drain released only some shards"
            );
        }
        // Everything this incarnation wrote is covered by final snapshots now.
        let next = self.inner.writer.next_lsn().seq;
        if released == shards.len() && self.inner.lease.raise_trunc(next).await.is_ok() {
            let _ = valka_wal::reader::truncate_before(
                &self.inner.store,
                self.node_id(),
                self.incarnation(),
                next,
            )
            .await;
        }
        self.inner
            .lease
            .set_state(valka_wal::LeaseState::Dead)
            .await?;
        let _ = self.inner.lease_stop.send(true);
        info!(released, "drained");
        self.inner.drained.send_replace(true);
        Ok(())
    }

    /// Turns true once a drain finished and the process should exit.
    pub fn drained_watch(&self) -> watch::Receiver<bool> {
        self.inner.drained.subscribe()
    }

    pub fn is_drained(&self) -> bool {
        *self.inner.drained.borrow()
    }

    /// Force a snapshot round now (tests, admin).
    pub async fn snapshot_now(&self) {
        self.inner.writer.sync().await.ok();
        self.snapshot_dirty_shards(true).await;
    }

    /// Wait until everything appended so far is durable.
    pub async fn sync(&self) -> Result<Lsn, ServerError> {
        Ok(self.inner.writer.sync().await?)
    }

    /// Stop background loops and lease renewal without flushing (tests: simulate
    /// `kill -9`).
    pub fn halt(&self) {
        let _ = self.inner.shutdown.send(true);
        let _ = self.inner.lease_stop.send(true);
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        let _ = self.lease_stop.send(true);
    }
}
