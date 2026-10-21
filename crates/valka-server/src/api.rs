//! The request layer both public APIs (REST and gRPC) go through.
//!
//! - **Id-routed commands** run on the node that owns the id's shard: locally when that is
//!   this node, otherwise forwarded over the internal RPC. A node that does not own the
//!   shard answers `NOT_OWNER` with a hint; after a couple of redirects the owner record
//!   in the bucket decides. Callers see `Unavailable` (retry) while a shard is moving.
//! - **Cluster-wide queries** (lists, counts, stats) go to every live node and are merged;
//!   a node that does not answer in time makes the result `partial`.
//!
//! See docs/wal/PHASE2.md §11.

use futures::future::join_all;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;
use valka_cluster::{Command, PeerClients, Query, Reply, RpcError, ScatterReply, WorkerInfo};
use valka_core::{ServerError, ShardId, TaskStatus};
use valka_dispatcher::DispatcherService;
use valka_dispatcher::TaskRouter;
use valka_engine::state::SignalStatus;
use valka_engine::{
    CheckpointView, CreateTask, DeadLetterView, DispatchInfo, DispatchableTask, Engine, NodeStats,
    ReattachOutcome, ResultOutcome, RunResult, RunView, ShardDetail, SignalView, TaskView,
};
use valka_wal::owners::{self, OwnerState};

/// Attempts at reaching a shard's owner before answering `Unavailable`.
const ROUTE_ATTEMPTS: usize = 5;

/// How long an owner waits for the node running a task to take a signal before answering
/// (it keeps trying in the background). Well below the peer call timeout, so a forwarded
/// `SendSignal` never times out at its sender because the run's node is unreachable.
const TELL_TIMEOUT: Duration = Duration::from_secs(2);

/// When a signal that could not be delivered (or was reset by a disconnect) is tried again
/// while its task runs. After the last try it waits for the task's next dispatch.
const SIGNAL_REDELIVERY: [Duration; 6] = [
    Duration::from_millis(200),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

/// The most a `Fetch` reply carries (it always carries at least one task): fetched tasks
/// over it go back to the owner's runnable index at once.
const FETCH_REPLY_BYTES: usize = 4 * 1024 * 1024;

/// How long (and how many) `SendSignal` request ids an owner remembers, so a forward
/// retried after a lost answer finds the signal its first attempt created.
const SIGNAL_REQUEST_TTL: Duration = Duration::from_secs(300);
const SIGNAL_REQUEST_MAX: usize = 65_536;

/// Signal request id -> the signal it created, oldest first.
#[derive(Default)]
struct SignalRequests {
    order: VecDeque<(tokio::time::Instant, String)>,
    ids: HashMap<String, String>,
}

impl SignalRequests {
    fn get(&self, request_id: &str) -> Option<String> {
        self.ids.get(request_id).cloned()
    }

    fn remember(&mut self, request_id: String, signal_id: String) {
        let now = tokio::time::Instant::now();
        while let Some((at, _)) = self.order.front() {
            if now.duration_since(*at) < SIGNAL_REQUEST_TTL && self.order.len() < SIGNAL_REQUEST_MAX
            {
                break;
            }
            if let Some((_, old)) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
        self.order.push_back((now, request_id.clone()));
        self.ids.insert(request_id, signal_id);
    }
}

/// Bytes of `value` as JSON, without building the JSON.
fn json_len<T: serde::Serialize>(value: &T) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut c = Count(0);
    let _ = serde_json::to_writer(&mut c, value);
    c.0
}

/// A cluster-wide answer, and whether some node did not contribute.
#[derive(Debug, Clone)]
pub struct Gathered<T> {
    pub value: T,
    /// Nodes that did not answer.
    pub missing: Vec<String>,
}

impl<T> Gathered<T> {
    pub fn partial(&self) -> bool {
        !self.missing.is_empty()
    }
}

#[derive(Clone)]
pub struct Api {
    inner: Arc<Inner>,
}

struct Inner {
    engine: Engine,
    dispatcher: DispatcherService,
    peers: PeerClients,
    /// shard -> node that last answered for it.
    owners: Mutex<HashMap<ShardId, String>>,
    scatter_timeout: Duration,
    /// See [`SignalRequests`].
    signal_requests: Mutex<SignalRequests>,
}

fn unexpected(r: Reply) -> ServerError {
    ServerError::Internal(format!("unexpected reply {r:?}"))
}

impl Api {
    pub fn new(engine: Engine, dispatcher: DispatcherService, peers: PeerClients) -> Self {
        // Peers size their messages like the internal listener does (see serve_internal).
        peers.set_max_message_bytes(valka_cluster::peer::max_message_bytes(
            engine.config().limits.max_payload_bytes,
        ));
        Self {
            inner: Arc::new(Inner {
                engine,
                dispatcher,
                peers,
                owners: Mutex::new(HashMap::new()),
                scatter_timeout: Duration::from_secs(3),
                signal_requests: Mutex::new(SignalRequests::default()),
            }),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.inner.engine
    }

    pub fn dispatcher(&self) -> &DispatcherService {
        &self.inner.dispatcher
    }

    pub fn node_id(&self) -> &str {
        self.inner.engine.node_id()
    }

    // ───────────────────────── id-routed ─────────────────────────

    pub async fn create_task(&self, req: CreateTask) -> Result<TaskView, ServerError> {
        Ok(self.create_task_replayed(req).await?.0)
    }

    /// Create a task. With an idempotency key already in use, the answer is the task that
    /// key made, and the flag says so (a retried create).
    pub async fn create_task_replayed(
        &self,
        req: CreateTask,
    ) -> Result<(TaskView, bool), ServerError> {
        let (id, shard) = self.inner.engine.place_task(&req).await?;
        if let Some(existing) = self.check_previous_partition(&req, shard).await? {
            return Ok((existing, true));
        }
        match self
            .route(Command::CreateTask {
                id: id.clone(),
                req,
            })
            .await?
        {
            Reply::Task(t) => {
                let replayed = t.id != id;
                Ok((t, replayed))
            }
            r => Err(unexpected(r)),
        }
    }

    /// After a queue's partition count changed, a keyed create also checks the key's old
    /// partition, so dedupe stays exact through a resize (PHASE2 §9). The window is the
    /// retention of terminal tasks: beyond it a duplicate key is allowed anyway.
    async fn check_previous_partition(
        &self,
        req: &CreateTask,
        shard: ShardId,
    ) -> Result<Option<TaskView>, ServerError> {
        let Some(key) = &req.idempotency_key else {
            return Ok(None);
        };
        let e = &self.inner.engine;
        let cfg = e.queue_config(&req.queue_name).await?;
        let Some(prev) = cfg.previous else {
            return Ok(None);
        };
        let window = chrono::Duration::seconds(e.config().wal.completed_retention_secs as i64);
        if e.clock().now() - prev.changed_at > window {
            return Ok(None);
        }
        let old = valka_core::partition_shard(
            &req.queue_name,
            valka_core::jump_hash(key, prev.partitions),
            e.num_shards(),
        );
        if old == shard {
            return Ok(None);
        }
        let probe = valka_core::new_id_in_shard(old).to_string();
        match self
            .route(Command::FindIdempotent {
                probe_id: probe,
                key: key.clone(),
                queue: Some(req.queue_name.clone()),
            })
            .await?
        {
            Reply::MaybeTask(found) => Ok(found),
            r => Err(unexpected(r)),
        }
    }

    pub async fn get_task(&self, task_id: &str) -> Result<Option<TaskView>, ServerError> {
        if self.inner.engine.shard_of(task_id).is_none() {
            return Ok(None);
        }
        match self
            .route(Command::GetTask {
                task_id: task_id.into(),
            })
            .await?
        {
            Reply::MaybeTask(t) => Ok(t),
            r => Err(unexpected(r)),
        }
    }

    pub async fn cancel_task(&self, task_id: &str, reason: &str) -> Result<TaskView, ServerError> {
        self.known(task_id)?;
        match self
            .route(Command::CancelTask {
                task_id: task_id.into(),
                reason: reason.into(),
                retry: false,
            })
            .await?
        {
            Reply::Task(t) => Ok(t),
            r => Err(unexpected(r)),
        }
    }

    pub async fn delete_task(&self, task_id: &str) -> Result<bool, ServerError> {
        if self.inner.engine.shard_of(task_id).is_none() {
            return Ok(false);
        }
        match self
            .route(Command::DeleteTask {
                task_id: task_id.into(),
            })
            .await?
        {
            Reply::Deleted(d) => Ok(d),
            r => Err(unexpected(r)),
        }
    }

    pub async fn send_signal(
        &self,
        task_id: &str,
        name: &str,
        payload: Option<serde_json::Value>,
    ) -> Result<(SignalView, bool), ServerError> {
        self.known(task_id)?;
        match self
            .route(Command::SendSignal {
                task_id: task_id.into(),
                name: name.into(),
                payload,
                request_id: Some(uuid::Uuid::now_v7().to_string()),
            })
            .await?
        {
            Reply::SignalSent { signal, delivered } => Ok((signal, delivered)),
            r => Err(unexpected(r)),
        }
    }

    pub async fn list_signals(
        &self,
        task_id: &str,
        status: Option<SignalStatus>,
    ) -> Result<Vec<SignalView>, ServerError> {
        if self.inner.engine.shard_of(task_id).is_none() {
            return Ok(Vec::new());
        }
        match self
            .route(Command::ListSignals {
                task_id: task_id.into(),
                status: status.map(|s| s.as_str().to_string()),
            })
            .await?
        {
            Reply::Signals(s) => Ok(s),
            r => Err(unexpected(r)),
        }
    }

    pub async fn runs(&self, task_id: &str) -> Result<Option<Vec<RunView>>, ServerError> {
        if self.inner.engine.shard_of(task_id).is_none() {
            return Ok(None);
        }
        match self
            .route(Command::Runs {
                task_id: task_id.into(),
            })
            .await?
        {
            Reply::Runs(r) => Ok(r),
            r => Err(unexpected(r)),
        }
    }

    pub async fn checkpoints(
        &self,
        task_id: &str,
    ) -> Result<Option<Vec<CheckpointView>>, ServerError> {
        if self.inner.engine.shard_of(task_id).is_none() {
            return Ok(None);
        }
        match self
            .route(Command::Checkpoints {
                task_id: task_id.into(),
            })
            .await?
        {
            Reply::Checkpoints(c) => Ok(c),
            r => Err(unexpected(r)),
        }
    }

    /// A shard's stats and per-queue counts, from the node serving it.
    pub async fn shard_detail(&self, shard: ShardId) -> Result<ShardDetail, ServerError> {
        let probe_id = valka_core::new_id_in_shard(shard).to_string();
        match self.route(Command::ShardDetail { probe_id }).await? {
            Reply::ShardDetail(d) => Ok(*d),
            r => Err(unexpected(r)),
        }
    }

    fn known(&self, task_id: &str) -> Result<ShardId, ServerError> {
        self.inner
            .engine
            .shard_of(task_id)
            .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))
    }

    /// Who probably serves `shard`, without any I/O.
    fn guess_owner(&self, shard: ShardId) -> Option<String> {
        let e = &self.inner.engine;
        if e.owns(shard) {
            return Some(e.node_id().to_string());
        }
        if let Some(n) = self.inner.owners.lock().get(&shard) {
            return Some(n.clone());
        }
        let eligible = e.eligible_nodes();
        e.desired_owner(shard, &eligible).map(str::to_string)
    }

    /// Who serves `shard` according to its owner record (one GET). `None` while the shard
    /// is unowned or released (a claim is due).
    async fn lookup_owner(&self, shard: ShardId) -> Option<String> {
        match owners::get(self.inner.engine.store(), shard).await {
            Ok(Some(v)) if v.record.state == OwnerState::Owned => Some(v.record.node),
            _ => None,
        }
    }

    /// The owner hint this node gives a caller it cannot serve.
    pub fn owner_hint(&self, shard: ShardId) -> Option<String> {
        self.guess_owner(shard)
            .filter(|n| n != self.node_id() || self.inner.engine.owns(shard))
    }

    fn peer_addr(&self, node: &str) -> Option<String> {
        self.inner
            .engine
            .live_peers()
            .into_iter()
            .find(|(n, _)| n == node)
            .and_then(|(_, l)| l.internal_addr)
    }

    /// Run `cmd` on the owner of its shard.
    async fn route(&self, mut cmd: Command) -> Result<Reply, ServerError> {
        let shard = self.known(cmd.routing_id())?;
        let me = self.node_id().to_string();
        let mut target = self.guess_owner(shard);
        let mut last = String::from("no owner known");
        for attempt in 0..ROUTE_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(50 << attempt.min(4))).await;
            }
            let Some(node) = target.take() else {
                target = self.lookup_owner(shard).await;
                if target.is_none() {
                    // Unowned or released: its desired owner is about to claim it.
                    let eligible = self.inner.engine.eligible_nodes();
                    target = self
                        .inner
                        .engine
                        .desired_owner(shard, &eligible)
                        .map(str::to_string);
                }
                continue;
            };
            if node == me {
                match self.execute_local(cmd.clone()).await {
                    Err(ServerError::NotOwner(_)) => {
                        last = format!("{me} does not serve shard {shard}");
                        target = self.lookup_owner(shard).await.filter(|n| *n != me);
                    }
                    r => return r,
                }
                continue;
            }
            let Some(addr) = self.peer_addr(&node) else {
                last = format!("{node} is not reachable (no live lease with an internal address)");
                target = self.lookup_owner(shard).await.filter(|n| *n != node);
                continue;
            };
            match self.inner.peers.execute(&addr, &cmd).await {
                Ok(r) => {
                    valka_core::metrics::record_forwarded(cmd.name(), "ok");
                    self.inner.owners.lock().insert(shard, node);
                    return Ok(r);
                }
                Err(RpcError::Server(e)) => {
                    valka_core::metrics::record_forwarded(cmd.name(), "error");
                    return Err(e);
                }
                Err(RpcError::NotOwner { hint, .. }) => {
                    valka_core::metrics::record_route_redirect();
                    self.inner.owners.lock().remove(&shard);
                    last = format!("{node} does not serve shard {shard}");
                    target = match hint.filter(|h| *h != node) {
                        Some(h) => Some(h),
                        None => self.lookup_owner(shard).await.filter(|n| *n != node),
                    };
                }
                Err(RpcError::Transport(m)) => {
                    valka_core::metrics::record_forwarded(cmd.name(), "unreachable");
                    self.inner.owners.lock().remove(&shard);
                    debug!(shard = %shard, node, error = %m, "forward failed");
                    last = m;
                    // The call may have run (a timeout): the next attempt says so.
                    cmd.mark_retry();
                    target = self.lookup_owner(shard).await;
                }
            }
        }
        Err(ServerError::Unavailable(format!(
            "shard {shard} is not reachable right now ({last}); retry"
        )))
    }

    // ───────────────────────── owner side ─────────────────────────

    fn ensure_readable(&self, task_id: &str) -> Result<ShardId, ServerError> {
        let shard = self.known(task_id)?;
        if !self.inner.engine.owns(shard) {
            return Err(ServerError::NotOwner(shard.0));
        }
        if !self.inner.engine.is_serving() {
            return Err(ServerError::Unavailable(
                "this node's lease is not current; retry".into(),
            ));
        }
        Ok(shard)
    }

    /// Run a command on this node: against its own shards, or (node commands) its own
    /// workers. Never forwards a shard command.
    pub async fn execute_local(&self, cmd: Command) -> Result<Reply, ServerError> {
        let e = &self.inner.engine;
        match cmd {
            Command::CreateTask { id, req } => Ok(Reply::Task(e.create_task_at(&id, req).await?)),
            Command::GetTask { task_id } => {
                self.ensure_readable(&task_id)?;
                Ok(Reply::MaybeTask(e.get_task(&task_id)))
            }
            Command::CancelTask {
                task_id,
                reason,
                retry,
            } => {
                let (view, running) = match e.cancel_task(&task_id, &reason).await {
                    Ok(r) => r,
                    Err(err @ ServerError::InvalidStatusTransition { .. }) if retry => {
                        // An earlier attempt whose answer was lost may have cancelled it.
                        return match e.get_task(&task_id) {
                            Some(t) if t.status == TaskStatus::Cancelled => {
                                e.sync().await?;
                                Ok(Reply::Task(t))
                            }
                            _ => Err(err),
                        };
                    }
                    Err(err) => return Err(err),
                };
                if let Some(loc) = running {
                    // Best effort and off the reply: a node that does not answer must not
                    // hold up the cancel (its worker finds out when its result is stale).
                    let me = self.clone();
                    tokio::spawn(async move {
                        me.tell_worker(&loc.node_id, Command::CancelOnWorker { task_id, reason })
                            .await;
                    });
                }
                Ok(Reply::Task(view))
            }
            Command::DeleteTask { task_id } => {
                self.ensure_readable(&task_id)?;
                Ok(Reply::Deleted(e.delete_task(&task_id).await?))
            }
            Command::SendSignal {
                task_id,
                name,
                payload,
                request_id,
            } => {
                self.ensure_readable(&task_id)?;
                if let Some(rid) = &request_id
                    && let Some(signal) = self.signal_for_request(rid, &task_id)
                {
                    // A forward retried after its answer was lost: answer with the signal
                    // the first attempt created (once durable), not with a second one.
                    e.sync().await?;
                    let delivered = signal.status != SignalStatus::Pending.as_str();
                    return Ok(Reply::SignalSent { signal, delivered });
                }
                let signal = e.send_signal(&task_id, &name, payload).await?;
                if let Some(rid) = request_id {
                    self.inner
                        .signal_requests
                        .lock()
                        .remember(rid, signal.id.clone());
                }
                let delivered = self.deliver_signal(&task_id, &signal).await;
                if !delivered && e.run_location(&task_id).is_some() {
                    // The run's worker may not have it yet (the dispatch is on its way) or
                    // its node did not answer: keep trying while the task runs.
                    self.spawn_signal_delivery(task_id, false);
                }
                Ok(Reply::SignalSent { signal, delivered })
            }
            Command::ListSignals { task_id, status } => {
                self.ensure_readable(&task_id)?;
                let status = status.as_deref().and_then(SignalStatus::parse);
                Ok(Reply::Signals(e.list_signals(&task_id, status)))
            }
            Command::Runs { task_id } => {
                self.ensure_readable(&task_id)?;
                Ok(Reply::Runs(e.runs_for_task(&task_id)))
            }
            Command::Checkpoints { task_id } => {
                self.ensure_readable(&task_id)?;
                Ok(Reply::Checkpoints(e.checkpoints_for_task(&task_id)))
            }
            Command::ShardDetail { probe_id } => {
                let shard = self.ensure_readable(&probe_id)?;
                let detail = e
                    .shard_detail(shard)
                    .ok_or(ServerError::NotOwner(shard.0))?;
                Ok(Reply::ShardDetail(Box::new(detail)))
            }
            Command::FindIdempotent {
                probe_id,
                key,
                queue,
            } => {
                let shard = self.ensure_readable(&probe_id)?;
                let found = match &queue {
                    Some(q) => e.find_idempotent(shard, q, &key),
                    // An older node: any queue's task with the key.
                    None => e.find_by_idempotency_key(shard, &key),
                };
                if found.is_some() {
                    // It is about to be someone's answer: only once it is durable.
                    e.sync().await?;
                }
                Ok(Reply::MaybeTask(found))
            }
            Command::Dispatch {
                task_id,
                worker_id,
                node_id,
                run_id,
            } => Ok(Reply::Dispatched(Box::new(e.dispatch_to(
                &task_id,
                &worker_id,
                &node_id,
                Some(&run_id),
            )?))),
            Command::ReturnRun { task_id, run_id } => {
                e.return_run(&task_id, &run_id).await?;
                Ok(Reply::Done)
            }
            Command::Unoffer { task_id } => {
                self.ensure_readable(&task_id)?;
                e.unoffer(&task_id);
                Ok(Reply::Done)
            }
            Command::ReportResult {
                task_id,
                run_id,
                result,
            } => {
                let shard = self.known(&task_id)?;
                if !e.owns(shard) {
                    return Err(ServerError::NotOwner(shard.0));
                }
                Ok(Reply::Result(
                    e.report_result(&task_id, &run_id, result).await,
                ))
            }
            Command::Checkpoint {
                task_id,
                run_id,
                step,
                output,
            } => Ok(Reply::Checkpoint(
                e.checkpoint(&task_id, &run_id, &step, output).await?,
            )),
            Command::SignalDelivered { signal_id } => {
                self.ensure_readable(&signal_id)?;
                e.signal_delivered(&signal_id);
                Ok(Reply::Done)
            }
            Command::SignalAcked { signal_id } => {
                self.ensure_readable(&signal_id)?;
                e.signal_acked(&signal_id);
                Ok(Reply::Done)
            }
            Command::Reattach {
                task_id,
                run_id,
                attempt,
                worker_id,
                node_id,
            } => {
                let outcome = e
                    .reattach(&task_id, &run_id, attempt, &worker_id, &node_id)
                    .await?;
                if outcome != ReattachOutcome::Stale {
                    // Signals its old connection left PENDING (reset when it went away, or
                    // never delivered) reach the run on its new one. The worker's node
                    // already counts the task as running, so the first try is immediate.
                    self.spawn_signal_delivery(task_id, true);
                }
                Ok(Reply::Reattached(outcome))
            }
            Command::ResetSignals { task_id } => {
                self.ensure_readable(&task_id)?;
                e.reset_signals(&task_id);
                // The worker may already run it elsewhere (it reconnected to another node
                // before this one noticed): try that node.
                if e.run_location(&task_id).is_some() {
                    self.spawn_signal_delivery(task_id, false);
                }
                Ok(Reply::Done)
            }
            cmd @ (Command::Heartbeat { .. }
            | Command::Fetch { .. }
            | Command::CancelOnWorker { .. }
            | Command::DeliverSignal { .. }
            | Command::Drain
            | Command::InvalidateQueue { .. }) => self.execute_node_command(cmd).await,
        }
    }

    /// Commands addressed to this node rather than routed by shard.
    async fn execute_node_command(&self, cmd: Command) -> Result<Reply, ServerError> {
        let e = &self.inner.engine;
        match cmd {
            Command::Heartbeat {
                task_ids,
                worker_id,
            } => {
                // Extend what this node owns; name the rest so the sender can re-route.
                let (mine, not_mine): (Vec<String>, Vec<String>) = task_ids
                    .into_iter()
                    .partition(|id| e.shard_of(id).is_some_and(|s| e.owns(s)));
                match worker_id.as_deref() {
                    Some(w) => e.heartbeat_from(w, &mine),
                    None => e.heartbeat(&mine),
                }
                Ok(Reply::NotOwned(not_mine))
            }
            Command::Fetch {
                queue,
                max,
                wait_ms,
            } => {
                let tasks = e
                    .fetch(
                        &queue,
                        max.min(1000),
                        Duration::from_millis(wait_ms.min(30_000)),
                    )
                    .await;
                Ok(Reply::Fetched(self.within_reply_budget(tasks)))
            }
            Command::CancelOnWorker { task_id, .. } => Ok(Reply::Delivered(
                self.inner.dispatcher.cancel_task_on_worker(&task_id).await,
            )),
            Command::DeliverSignal { signal } => Ok(Reply::Delivered(
                self.inner
                    .dispatcher
                    .send_signal_to_worker(
                        &signal.task_id,
                        valka_proto::TaskSignal {
                            signal_id: signal.id.clone(),
                            task_id: signal.task_id.clone(),
                            signal_name: signal.signal_name.clone(),
                            payload: signal
                                .payload
                                .clone()
                                .map(|v| v.to_string())
                                .unwrap_or_default(),
                            timestamp_ms: signal.created_at.timestamp_millis(),
                        },
                    )
                    .await,
            )),
            Command::Drain => {
                let e = e.clone();
                tokio::spawn(async move {
                    if let Err(err) = e.drain().await {
                        tracing::error!(error = %err, "drain failed");
                    }
                });
                Ok(Reply::Done)
            }
            Command::InvalidateQueue { queue } => {
                e.invalidate_queue(&queue);
                Ok(Reply::Done)
            }
            other => Err(ServerError::Internal(format!(
                "{} is not a node command",
                other.name()
            ))),
        }
    }

    /// Send a node command to `node` (this node included). `None` if it could not be
    /// delivered.
    pub async fn send_to_node(&self, node: &str, cmd: Command) -> Option<Reply> {
        if node == self.node_id() {
            return self.execute_node_command(cmd).await.ok();
        }
        let addr = self.peer_addr(node)?;
        match self.inner.peers.execute(&addr, &cmd).await {
            Ok(r) => Some(r),
            Err(e) => {
                debug!(node, cmd = cmd.name(), error = %e, "node command failed");
                None
            }
        }
    }

    /// Tell the node a run lives on about it (cancel, signal). Best effort: a worker that
    /// misses a cancel finds out when its result comes back stale.
    async fn tell_worker(&self, node: &str, cmd: Command) -> Option<Reply> {
        self.send_to_node(node, cmd).await
    }

    /// The signal a `SendSignal` with this request id created, if this node remembers it.
    fn signal_for_request(&self, request_id: &str, task_id: &str) -> Option<SignalView> {
        let id = self.inner.signal_requests.lock().get(request_id)?;
        self.inner
            .engine
            .list_signals(task_id, None)
            .into_iter()
            .find(|s| s.id == id)
    }

    /// Hand `signal` to the worker running its task, waiting at most `TELL_TIMEOUT`; mark
    /// it delivered if that worked.
    async fn deliver_signal(&self, task_id: &str, signal: &SignalView) -> bool {
        let e = &self.inner.engine;
        let Some(loc) = e.run_location(task_id) else {
            return false;
        };
        let cmd = Command::DeliverSignal {
            signal: signal.clone(),
        };
        let delivered = tokio::time::timeout(TELL_TIMEOUT, self.tell_worker(&loc.node_id, cmd))
            .await
            .ok()
            .flatten()
            .is_some_and(|r| r == Reply::Delivered(true));
        if delivered {
            e.signal_delivered(&signal.id);
        }
        delivered
    }

    /// Deliver a running task's PENDING signals. True once nothing is left to do here (all
    /// delivered, or the task no longer runs or is no longer this node's).
    async fn deliver_pending_signals(&self, task_id: &str) -> bool {
        let e = &self.inner.engine;
        if !e.shard_of(task_id).is_some_and(|s| e.owns(s)) || e.run_location(task_id).is_none() {
            return true;
        }
        let mut all = true;
        for signal in e.pending_signals(task_id) {
            all &= self.deliver_signal(task_id, &signal).await;
        }
        all
    }

    /// Keep delivering `task_id`'s PENDING signals in the background (see
    /// `SIGNAL_REDELIVERY`) while it runs. PENDING signals otherwise only travel with a
    /// dispatch, so one missed while the task runs would wait for its next attempt.
    fn spawn_signal_delivery(&self, task_id: String, immediately: bool) {
        let me = self.clone();
        tokio::spawn(async move {
            if immediately && me.deliver_pending_signals(&task_id).await {
                return;
            }
            for delay in SIGNAL_REDELIVERY {
                tokio::time::sleep(delay).await;
                if me.deliver_pending_signals(&task_id).await {
                    return;
                }
            }
            debug!(
                task_id,
                "signals still pending; the task's next dispatch carries them"
            );
        });
    }

    /// Fetched tasks up to `FETCH_REPLY_BYTES` (at least one); the rest are runnable again.
    fn within_reply_budget(&self, tasks: Vec<DispatchableTask>) -> Vec<DispatchableTask> {
        let mut used = 0usize;
        let mut out = Vec::with_capacity(tasks.len());
        for t in tasks {
            let size = json_len(&t);
            if !out.is_empty() && used.saturating_add(size) > FETCH_REPLY_BYTES {
                self.inner.engine.unoffer(&t.task_id);
                continue;
            }
            used = used.saturating_add(size);
            out.push(t);
        }
        out
    }

    /// Group task ids by the node this one believes owns them.
    fn by_owner(&self, ids: &[String]) -> HashMap<String, Vec<String>> {
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for id in ids {
            let Some(shard) = self.inner.engine.shard_of(id) else {
                continue;
            };
            if let Some(owner) = self.guess_owner(shard) {
                out.entry(owner).or_default().push(id.clone());
            }
        }
        out
    }

    /// Heartbeats for running tasks, batched per owner. Ids an owner disowns are looked
    /// up in the bucket and sent again.
    pub async fn route_heartbeats(&self, worker_id: Option<&str>, task_ids: &[String]) {
        let worker_id = worker_id.map(str::to_string);
        let mut retry = Vec::new();
        for (node, ids) in self.by_owner(task_ids) {
            match self
                .send_to_node(
                    &node,
                    Command::Heartbeat {
                        task_ids: ids.clone(),
                        worker_id: worker_id.clone(),
                    },
                )
                .await
            {
                Some(Reply::NotOwned(rest)) => retry.extend(rest),
                Some(_) => {}
                None => retry.extend(ids),
            }
        }
        if retry.is_empty() {
            return;
        }
        let mut again: HashMap<String, Vec<String>> = HashMap::new();
        for id in retry {
            let Some(shard) = self.inner.engine.shard_of(&id) else {
                continue;
            };
            self.inner.owners.lock().remove(&shard);
            if let Some(owner) = self.lookup_owner(shard).await {
                self.inner.owners.lock().insert(shard, owner.clone());
                again.entry(owner).or_default().push(id);
            }
        }
        for (node, ids) in again {
            let cmd = Command::Heartbeat {
                task_ids: ids,
                worker_id: worker_id.clone(),
            };
            self.send_to_node(&node, cmd).await;
        }
    }

    /// Answer a query from this node's own shards.
    pub async fn scatter_local(&self, q: Query) -> Result<ScatterReply, ServerError> {
        let e = &self.inner.engine;
        Ok(match q {
            Query::ListTasks {
                queue,
                status,
                limit,
            } => ScatterReply::Tasks(e.list_tasks(queue.as_deref(), status, limit, 0)),
            Query::CountTasks { queue, status } => {
                ScatterReply::Count(e.count_tasks(queue.as_deref(), status))
            }
            Query::DeadLetters { queue, limit } => {
                ScatterReply::DeadLetters(e.list_dead_letters(queue.as_deref(), limit, 0))
            }
            Query::Workers => ScatterReply::Workers(self.local_workers()),
            Query::NodeStats => ScatterReply::NodeStats(Box::new(e.stats())),
            Query::Shards => ScatterReply::Shards(
                e.shard_stats()
                    .into_iter()
                    .filter(|s| s.owner.is_some())
                    .collect(),
            ),
            Query::Queues => ScatterReply::Queues(e.queues()),
            Query::ClearTasks => ScatterReply::Cleared(e.clear_all_tasks().await?),
        })
    }

    pub fn local_workers(&self) -> Vec<WorkerInfo> {
        let node = self.node_id().to_string();
        self.inner
            .dispatcher
            .workers()
            .iter()
            .map(|entry| {
                let h = entry.value();
                WorkerInfo {
                    id: h.worker_id.0.clone(),
                    node_id: node.clone(),
                    name: h.worker_name.clone(),
                    queues: h.queues.clone(),
                    concurrency: h.concurrency,
                    active_tasks: h.active_tasks.len(),
                    last_heartbeat: h.last_heartbeat,
                    connected_at: h.connected_at,
                }
            })
            .collect()
    }

    // ───────────────────────── scatter-gather ─────────────────────────

    /// Ask this node and every live peer; `missing` lists peers that did not answer.
    async fn gather(&self, q: Query) -> Result<(Vec<ScatterReply>, Vec<String>), ServerError> {
        let local = self.scatter_local(q.clone()).await?;
        let peers: Vec<(String, Option<String>)> = self
            .inner
            .engine
            .live_peers()
            .into_iter()
            .map(|(n, l)| (n, l.internal_addr))
            .collect();
        let timeout = self.inner.scatter_timeout;
        let calls = peers.into_iter().map(|(node, addr)| {
            let q = q.clone();
            let peers = self.inner.peers.clone();
            async move {
                let Some(addr) = addr else {
                    return Err(node);
                };
                match tokio::time::timeout(timeout, peers.scatter(&addr, &q)).await {
                    Ok(Ok(r)) => Ok(r),
                    _ => Err(node),
                }
            }
        });
        let mut replies = vec![local];
        let mut missing = Vec::new();
        for r in join_all(calls).await {
            match r {
                Ok(r) => replies.push(r),
                Err(node) => missing.push(node),
            }
        }
        // A node whose lease went quiet (suspect, not yet declared dead) is not asked, and
        // nobody else serves its shards yet: the answer lacks them.
        for m in self.inner.engine.members() {
            if m.suspect && !missing.contains(&m.node_id) {
                missing.push(m.node_id);
            }
        }
        Ok((replies, missing))
    }

    /// Newest first, paged by offset across every node.
    pub async fn list_tasks(
        &self,
        queue: Option<&str>,
        status: Option<TaskStatus>,
        limit: usize,
        offset: usize,
    ) -> Result<Gathered<Vec<TaskView>>, ServerError> {
        let (replies, missing) = self
            .gather(Query::ListTasks {
                queue: queue.map(str::to_string),
                status,
                limit: limit + offset,
            })
            .await?;
        let mut all: Vec<TaskView> = replies
            .into_iter()
            .flat_map(|r| match r {
                ScatterReply::Tasks(t) => t,
                _ => Vec::new(),
            })
            .collect();
        all.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        all.dedup_by(|a, b| a.id == b.id);
        Ok(Gathered {
            value: all.into_iter().skip(offset).take(limit).collect(),
            missing,
        })
    }

    pub async fn count_tasks(
        &self,
        queue: Option<&str>,
        status: Option<TaskStatus>,
    ) -> Result<Gathered<usize>, ServerError> {
        let (replies, missing) = self
            .gather(Query::CountTasks {
                queue: queue.map(str::to_string),
                status,
            })
            .await?;
        Ok(Gathered {
            value: replies
                .into_iter()
                .map(|r| match r {
                    ScatterReply::Count(n) => n,
                    _ => 0,
                })
                .sum(),
            missing,
        })
    }

    pub async fn dead_letters(
        &self,
        queue: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Gathered<Vec<DeadLetterView>>, ServerError> {
        let (replies, missing) = self
            .gather(Query::DeadLetters {
                queue: queue.map(str::to_string),
                limit: limit + offset,
            })
            .await?;
        let mut all: Vec<DeadLetterView> = replies
            .into_iter()
            .flat_map(|r| match r {
                ScatterReply::DeadLetters(d) => d,
                _ => Vec::new(),
            })
            .collect();
        all.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(Gathered {
            value: all.into_iter().skip(offset).take(limit).collect(),
            missing,
        })
    }

    pub async fn workers(&self) -> Result<Gathered<Vec<WorkerInfo>>, ServerError> {
        let (replies, missing) = self.gather(Query::Workers).await?;
        Ok(Gathered {
            value: replies
                .into_iter()
                .flat_map(|r| match r {
                    ScatterReply::Workers(w) => w,
                    _ => Vec::new(),
                })
                .collect(),
            missing,
        })
    }

    pub async fn node_stats(&self) -> Result<Gathered<Vec<NodeStats>>, ServerError> {
        let (replies, missing) = self.gather(Query::NodeStats).await?;
        Ok(Gathered {
            value: replies
                .into_iter()
                .filter_map(|r| match r {
                    ScatterReply::NodeStats(s) => Some(*s),
                    _ => None,
                })
                .collect(),
            missing,
        })
    }

    /// One row per served shard, from every node.
    pub async fn shard_map(&self) -> Result<Gathered<Vec<valka_engine::ShardStats>>, ServerError> {
        let (replies, missing) = self.gather(Query::Shards).await?;
        Ok(Gathered {
            value: replies
                .into_iter()
                .flat_map(|r| match r {
                    ScatterReply::Shards(s) => s,
                    _ => Vec::new(),
                })
                .collect(),
            missing,
        })
    }

    pub async fn queues(&self) -> Result<Gathered<Vec<String>>, ServerError> {
        let (replies, missing) = self.gather(Query::Queues).await?;
        let mut all: Vec<String> = replies
            .into_iter()
            .flat_map(|r| match r {
                ScatterReply::Queues(q) => q,
                _ => Vec::new(),
            })
            .collect();
        all.sort();
        all.dedup();
        Ok(Gathered {
            value: all,
            missing,
        })
    }

    pub async fn clear_all_tasks(&self) -> Result<Gathered<usize>, ServerError> {
        let (replies, missing) = self.gather(Query::ClearTasks).await?;
        Ok(Gathered {
            value: replies
                .into_iter()
                .map(|r| match r {
                    ScatterReply::Cleared(n) => n,
                    _ => 0,
                })
                .sum(),
            missing,
        })
    }
}

/// The worker side of tasks: each call goes to the task's owner (this node or another).
#[async_trait::async_trait]
impl TaskRouter for Api {
    async fn dispatch(&self, task_id: &str, worker_id: &str) -> Result<DispatchInfo, ServerError> {
        let shard = self.known(task_id)?;
        let e = &self.inner.engine;
        if e.owns(shard) {
            return e.dispatch(task_id, worker_id);
        }
        // The run id is minted here so a retried forward is idempotent at the owner.
        let cmd = Command::Dispatch {
            task_id: task_id.into(),
            worker_id: worker_id.into(),
            node_id: self.node_id().into(),
            run_id: valka_core::new_id_in_shard(shard).to_string(),
        };
        match self.route(cmd).await? {
            Reply::Dispatched(info) => Ok(*info),
            r => Err(unexpected(r)),
        }
    }

    async fn return_run(&self, task_id: &str, run_id: &str) {
        let cmd = Command::ReturnRun {
            task_id: task_id.into(),
            run_id: run_id.into(),
        };
        if let Err(e) = self.route(cmd).await {
            debug!(task_id, run_id, error = %e, "return failed; the lease expires instead");
        }
    }

    async fn unoffer(&self, task_id: &str) {
        let cmd = Command::Unoffer {
            task_id: task_id.into(),
        };
        if let Err(e) = self.route(cmd).await {
            debug!(task_id, error = %e, "unoffer failed; the reservation lapses instead");
        }
    }

    async fn report_result(&self, task_id: &str, run_id: &str, result: RunResult) -> ResultOutcome {
        if self.inner.engine.shard_of(task_id).is_none() {
            return ResultOutcome::Stale;
        }
        let cmd = Command::ReportResult {
            task_id: task_id.into(),
            run_id: run_id.into(),
            result,
        };
        match self.route(cmd).await {
            Ok(Reply::Result(o)) => o,
            // The owner could not be reached: the worker keeps the result and resends.
            _ => ResultOutcome::Retry,
        }
    }

    async fn heartbeat(&self, worker_id: &str, task_ids: &[String]) {
        let e = &self.inner.engine;
        let (mine, remote): (Vec<String>, Vec<String>) = task_ids
            .iter()
            .cloned()
            .partition(|id| e.shard_of(id).is_some_and(|s| e.owns(s)));
        // Only this worker's runs: a run that went to another worker must not be kept
        // alive by the one still busy with the expired run.
        e.heartbeat_from(worker_id, &mine);
        if !remote.is_empty() {
            // Off the worker's stream: a slow owner must not hold up its other messages.
            let (me, worker_id) = (self.clone(), worker_id.to_string());
            tokio::spawn(async move { me.route_heartbeats(Some(&worker_id), &remote).await });
        }
    }

    async fn checkpoint(
        &self,
        task_id: &str,
        run_id: &str,
        step: &str,
        output: serde_json::Value,
    ) -> Result<CheckpointView, ServerError> {
        self.known(task_id)?;
        let cmd = Command::Checkpoint {
            task_id: task_id.into(),
            run_id: run_id.into(),
            step: step.into(),
            output,
        };
        match self.route(cmd).await? {
            Reply::Checkpoint(c) => Ok(c),
            r => Err(unexpected(r)),
        }
    }

    async fn signal_delivered(&self, signal_id: &str) {
        let _ = self
            .route(Command::SignalDelivered {
                signal_id: signal_id.into(),
            })
            .await;
    }

    async fn signal_acked(&self, signal_id: &str) {
        let _ = self
            .route(Command::SignalAcked {
                signal_id: signal_id.into(),
            })
            .await;
    }

    async fn reset_signals(&self, task_id: &str) {
        let _ = self
            .route(Command::ResetSignals {
                task_id: task_id.into(),
            })
            .await;
    }

    async fn reattach(
        &self,
        task_id: &str,
        run_id: &str,
        attempt: i32,
        worker_id: &str,
    ) -> Result<valka_engine::ReattachOutcome, ServerError> {
        if self.inner.engine.shard_of(task_id).is_none() {
            return Ok(valka_engine::ReattachOutcome::Stale);
        }
        let cmd = Command::Reattach {
            task_id: task_id.into(),
            run_id: run_id.into(),
            attempt,
            worker_id: worker_id.into(),
            node_id: self.node_id().into(),
        };
        match self.route(cmd).await {
            Ok(Reply::Reattached(o)) => Ok(o),
            // A task the cluster does not know: it was deleted, the run is stale.
            Err(ServerError::TaskNotFound(_)) => Ok(valka_engine::ReattachOutcome::Stale),
            Ok(r) => Err(unexpected(r)),
            Err(e) => Err(e),
        }
    }
}

impl Api {
    /// Send a node command to every live node, this one included. Returns the nodes that
    /// did not take it.
    pub async fn broadcast(&self, cmd: Command) -> Vec<String> {
        let mut nodes = vec![self.node_id().to_string()];
        nodes.extend(self.inner.engine.live_peers().into_iter().map(|(n, _)| n));
        let mut missed = Vec::new();
        for n in nodes {
            if self.send_to_node(&n, cmd.clone()).await.is_none() {
                missed.push(n);
            }
        }
        missed
    }

    /// Live nodes other than this one that own partitions of `queue`.
    pub async fn remote_owners(&self, queue: &str) -> Vec<String> {
        let e = &self.inner.engine;
        let Ok(cfg) = e.queue_config(queue).await else {
            return Vec::new();
        };
        let me = self.node_id();
        // Partitions a shrink removed still hold tasks until they drain: fetch from them too.
        let mut out: Vec<String> = (0..cfg.served_partitions())
            .map(|p| valka_core::partition_shard(queue, p, e.num_shards()))
            .filter(|s| !e.owns(*s))
            .filter_map(|s| self.guess_owner(s))
            .filter(|n| n != me)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn peers(&self) -> &PeerClients {
        &self.inner.peers
    }

    /// The internal address of a live peer.
    pub fn peer_internal_addr(&self, node: &str) -> Option<String> {
        self.peer_addr(node)
    }
}
