use crate::heartbeat;
use crate::router::{LocalRouter, TaskRouter};
use crate::worker_handle::WorkerHandle;
use dashmap::DashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};
use valka_core::{NodeId, PartitionId, WorkerId};
use valka_engine::{DispatchInfo, Engine, ResultOutcome, RunResult};
use valka_matching::MatchingService;
use valka_matching::partition::TaskEnvelope;
use valka_proto::{
    Heartbeat, LogBatch, ResultAck, ResultStatus, SignalAck, StepCheckpoint, TaskAssignment,
    TaskCancellation, TaskResult, TaskSignal, WorkerResponse, worker_response,
};
use valka_wal::logstore::LogLine;

/// Most slots a worker registers in one round of its match loop.
const MAX_SLOTS_PER_ROUND: i32 = 32;

/// How long a reattach whose owner is unreachable (its shard is moving) keeps being retried
/// while the worker still runs the task.
const REATTACH_RETRY_FOR: Duration = Duration::from_secs(60);

/// The dispatcher manages all connected workers and their gRPC streams.
#[derive(Clone)]
pub struct DispatcherService {
    workers: Arc<DashMap<String, WorkerHandle>>,
    matching: MatchingService,
    engine: Engine,
    /// Where the worker side of each task goes (its owner). Local engine by default.
    router: Arc<RwLock<Arc<dyn TaskRouter>>>,
    node_id: NodeId,
    log_tx: mpsc::Sender<LogLine>,
}

impl DispatcherService {
    pub fn new(
        matching: MatchingService,
        engine: Engine,
        node_id: NodeId,
        log_tx: mpsc::Sender<LogLine>,
    ) -> Self {
        Self {
            workers: Arc::new(DashMap::new()),
            matching,
            router: Arc::new(RwLock::new(Arc::new(LocalRouter(engine.clone())))),
            engine,
            node_id,
            log_tx,
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn matching(&self) -> &MatchingService {
        &self.matching
    }

    /// Route the worker side of tasks through `router` (the server's cluster router).
    pub fn set_router(&self, router: Arc<dyn TaskRouter>) {
        *self.router.write().unwrap() = router;
    }

    pub fn router(&self) -> Arc<dyn TaskRouter> {
        self.router.read().unwrap().clone()
    }

    pub async fn register_worker(&self, handle: WorkerHandle) {
        let worker_id = handle.worker_id.clone();
        self.workers.insert(worker_id.0.clone(), handle);
        valka_core::metrics::set_active_workers(self.workers.len() as f64);
    }

    pub async fn deregister_worker(&self, worker_id: &WorkerId) {
        if let Some((_, handle)) = self.workers.remove(worker_id.as_ref()) {
            self.matching.deregister_worker(worker_id);
            // Delivered-but-unacked signals go back to PENDING for redelivery.
            let router = self.router();
            for task_id in &handle.active_tasks {
                router.reset_signals(task_id).await;
            }
            info!(
                worker_id = %worker_id,
                active_tasks = handle.active_tasks.len(),
                "Worker deregistered"
            );
            // Active tasks are reclaimed by lease expiry in the engine.
        }
        valka_core::metrics::set_active_workers(self.workers.len() as f64);
    }

    /// A (re)connecting worker still runs these runs. Each is reattached to this node,
    /// adopted if its dispatch record was lost, or cancelled on the worker if stale. A run
    /// whose owner cannot be reached right now (its shard is moving) is retried in the
    /// background while the worker still runs it: until the owner hears about it, cancels
    /// and signals go to the run's old node, and a lost dispatch would not be adopted.
    pub async fn reattach_running(
        &self,
        worker_id: &WorkerId,
        running: &[valka_proto::RunningTask],
        tx: &mpsc::Sender<WorkerResponse>,
    ) {
        let router = self.router();
        for r in running {
            if let Some(mut h) = self.workers.get_mut(worker_id.as_ref()) {
                h.assign_task(r.task_id.clone());
            }
            match router
                .reattach(&r.task_id, &r.task_run_id, r.attempt_number, &worker_id.0)
                .await
            {
                Ok(outcome) => self.reattached(worker_id, r, outcome, tx).await,
                Err(e) => {
                    valka_core::metrics::record_reattach("retrying");
                    warn!(task_id = %r.task_id, error = %e, "reattach failed; retrying in the background");
                    let (me, worker_id, r, tx) =
                        (self.clone(), worker_id.clone(), r.clone(), tx.clone());
                    tokio::spawn(async move { me.retry_reattach(worker_id, r, tx).await });
                }
            }
        }
    }

    async fn retry_reattach(
        &self,
        worker_id: WorkerId,
        r: valka_proto::RunningTask,
        tx: mpsc::Sender<WorkerResponse>,
    ) {
        let deadline = tokio::time::Instant::now() + REATTACH_RETRY_FOR;
        let mut delay = Duration::from_millis(500);
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(5));
            let still_running = self
                .workers
                .get(worker_id.as_ref())
                .is_some_and(|h| h.active_tasks.contains(&r.task_id));
            if !still_running || tx.is_closed() {
                return;
            }
            match self
                .router()
                .reattach(&r.task_id, &r.task_run_id, r.attempt_number, &worker_id.0)
                .await
            {
                Ok(outcome) => return self.reattached(&worker_id, &r, outcome, &tx).await,
                Err(e) => debug!(task_id = %r.task_id, error = %e, "reattach still failing"),
            }
        }
        valka_core::metrics::record_reattach("failed");
        warn!(task_id = %r.task_id, "reattach gave up: the task's owner stayed unreachable");
    }

    async fn reattached(
        &self,
        worker_id: &WorkerId,
        r: &valka_proto::RunningTask,
        outcome: valka_engine::ReattachOutcome,
        tx: &mpsc::Sender<WorkerResponse>,
    ) {
        valka_core::metrics::record_reattach(match outcome {
            valka_engine::ReattachOutcome::Reattached => "reattached",
            valka_engine::ReattachOutcome::Adopted => "adopted",
            valka_engine::ReattachOutcome::Stale => "stale",
        });
        if outcome != valka_engine::ReattachOutcome::Stale {
            debug!(worker_id = %worker_id, task_id = %r.task_id, outcome = ?outcome, "run reattached");
            return;
        }
        info!(worker_id = %worker_id, task_id = %r.task_id, "stale run on reconnect: cancelling it");
        if let Some(mut h) = self.workers.get_mut(worker_id.as_ref()) {
            h.complete_task(&r.task_id);
        }
        let cancel = WorkerResponse {
            response: Some(worker_response::Response::TaskCancellation(
                TaskCancellation {
                    task_id: r.task_id.clone(),
                    reason: "stale run: it ended or was superseded".to_string(),
                },
            )),
        };
        let _ = tx.send(cancel).await;
    }

    /// The worker is draining: no new tasks, but its stream stays open for results.
    pub fn stop_dispatching(&self, worker_id: &WorkerId) {
        self.matching.deregister_worker(worker_id);
    }

    /// Background loop: register as waiting in matching service, receive tasks, push to worker.
    ///
    /// Each round registers one slot per free concurrency slot (up to
    /// `MAX_SLOTS_PER_ROUND`) on every queue, spread over the partitions, waits for the first
    /// task, then takes whatever the other slots received in the meantime.
    pub async fn run_worker_match_loop(&self, worker_id: WorkerId, queues: Vec<String>) {
        let num_partitions = self.matching.config().num_partitions.max(1) as usize;
        let Some(slot_freed) = self
            .workers
            .get(worker_id.as_ref())
            .map(|h| h.slot_freed.clone())
        else {
            return;
        };
        let mut round = 0usize;

        loop {
            let available = {
                match self.workers.get(worker_id.as_ref()) {
                    Some(handle) => handle.available_slots(),
                    None => return, // Worker disconnected
                }
            };

            if available <= 0 || queues.is_empty() {
                // Woken when a result frees a slot; the timeout notices a disconnect.
                let _ =
                    tokio::time::timeout(Duration::from_millis(250), slot_freed.notified()).await;
                continue;
            }

            let slots = available.min(MAX_SLOTS_PER_ROUND) as usize;
            let mut receivers: Vec<oneshot::Receiver<TaskEnvelope>> =
                Vec::with_capacity(slots * queues.len());
            for queue in &queues {
                for i in 0..slots {
                    let partition_id = PartitionId(((round + i) % num_partitions) as i32);
                    receivers.push(self.matching.register_worker(
                        queue,
                        partition_id,
                        worker_id.clone(),
                    ));
                }
            }
            round = round.wrapping_add(1);
            // A worker is waiting: let the local feeder offer it pending tasks right away.
            self.engine.wake_feeder();

            let (first, index, _) = futures::future::select_all(receivers.iter_mut()).await;
            let mut matched: Vec<TaskEnvelope> = first.into_iter().collect();
            // Close the other slots before reading them, so no task can land in a slot
            // after it was read and be dropped with it.
            for (i, rx) in receivers.iter_mut().enumerate() {
                if i == index {
                    continue;
                }
                rx.close();
                if let Ok(envelope) = rx.try_recv() {
                    matched.push(envelope);
                }
            }
            drop(receivers);
            if matched.is_empty() {
                debug!(worker_id = %worker_id, "Match channel closed");
                continue;
            }

            // A worker on several queues may receive more than it has free slots: the
            // rest go back to their owners' pending index (they may be another node's).
            for envelope in matched {
                let free = self
                    .workers
                    .get(worker_id.as_ref())
                    .is_some_and(|h| h.available_slots() > 0);
                if free {
                    self.dispatch_to_worker(&worker_id, envelope).await;
                } else {
                    self.router().unoffer(&envelope.task_id).await;
                }
            }
        }
    }

    async fn dispatch_to_worker(&self, worker_id: &WorkerId, envelope: TaskEnvelope) {
        let router = self.router();
        // Is the worker still here? If not, hand the task back before recording anything.
        if !self.workers.contains_key(worker_id.as_ref()) {
            router.unoffer(&envelope.task_id).await;
            return;
        }

        // Record the dispatch (RUNNING + run + lease) with the task's owner.
        let info: DispatchInfo = match router.dispatch(&envelope.task_id, &worker_id.0).await {
            Ok(info) => info,
            Err(e) => {
                // Cancelled / deleted / already running / owner briefly not serving: hand the
                // offer back so a PENDING task is runnable again now (a no-op otherwise).
                debug!(task_id = %envelope.task_id, error = %e, "dispatch refused");
                router.unoffer(&envelope.task_id).await;
                return;
            }
        };

        valka_core::metrics::record_dispatch_latency(&info.task.queue_name, 0.0);

        let assignment = TaskAssignment {
            task_id: info.task.task_id.clone(),
            task_run_id: info.run_id.clone(),
            queue_name: info.task.queue_name.clone(),
            task_name: info.task.task_name.clone(),
            input: info.task.input.map(|v| v.to_string()).unwrap_or_default(),
            attempt_number: info.attempt,
            timeout_seconds: info.task.timeout_seconds,
            metadata: info.task.metadata.to_string(),
            checkpoints: info
                .checkpoints
                .into_iter()
                .map(|c| StepCheckpoint {
                    step: c.step,
                    output: c.output.to_string(),
                    attempt_number: c.attempt_number,
                    created_at_ms: c.created_at.timestamp_millis(),
                })
                .collect(),
        };

        let tx = {
            let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) else {
                // Worker vanished between the check and now: the run never started.
                router.return_run(&envelope.task_id, &info.run_id).await;
                return;
            };
            handle.assign_task(envelope.task_id.clone());
            handle.response_tx.clone()
        };
        let response = WorkerResponse {
            response: Some(worker_response::Response::TaskAssignment(assignment)),
        };
        if tx.send(response).await.is_err() {
            warn!(worker_id = %worker_id, "Failed to send task assignment - worker disconnected");
            if let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) {
                handle.complete_task(&envelope.task_id);
            }
            router.return_run(&envelope.task_id, &info.run_id).await;
            return;
        }

        // Deliver signals sent before the run started.
        for sig in info.pending_signals {
            let signal_response = WorkerResponse {
                response: Some(worker_response::Response::TaskSignal(TaskSignal {
                    signal_id: sig.id.clone(),
                    task_id: sig.task_id,
                    signal_name: sig.signal_name,
                    payload: sig.payload.map(|v| v.to_string()).unwrap_or_default(),
                    timestamp_ms: sig.created_at.timestamp_millis(),
                })),
            };
            if tx.send(signal_response).await.is_ok() {
                router.signal_delivered(&sig.id).await;
            }
        }
    }

    /// Record a worker's result and answer with a `ResultAck`. The worker keeps the result
    /// until the answer is APPLIED or STALE, so a lost ack only costs a resend.
    pub async fn handle_task_result(&self, worker_id: &WorkerId, result: TaskResult) {
        let tx = self.workers.get_mut(worker_id.as_ref()).map(|mut handle| {
            handle.complete_task(&result.task_id);
            handle.response_tx.clone()
        });

        let run_result = if result.success {
            RunResult::Completed(if result.output.is_empty() {
                None
            } else {
                serde_json::from_str(&result.output).ok()
            })
        } else {
            RunResult::Failed {
                error: result.error_message.clone(),
                retryable: result.retryable,
            }
        };
        let outcome = self
            .router()
            .report_result(&result.task_id, &result.task_run_id, run_result)
            .await;
        let status = match outcome {
            ResultOutcome::Applied => ResultStatus::Applied,
            ResultOutcome::Stale => {
                warn!(
                    task_id = %result.task_id,
                    task_run_id = %result.task_run_id,
                    "stale task result: the run already ended another way"
                );
                ResultStatus::Stale
            }
            ResultOutcome::Retry => ResultStatus::Retry,
        };
        if let Some(tx) = tx {
            let ack = WorkerResponse {
                response: Some(worker_response::Response::ResultAck(ResultAck {
                    task_id: result.task_id,
                    task_run_id: result.task_run_id,
                    status: status as i32,
                    message: String::new(),
                })),
            };
            let _ = tx.send(ack).await;
        }
    }

    pub async fn handle_heartbeat(&self, worker_id: &WorkerId, heartbeat: Heartbeat) {
        if let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) {
            handle.update_heartbeat();
        }
        self.router().heartbeat(&heartbeat.active_task_ids).await;
    }

    pub async fn handle_log_batch(&self, _worker_id: &WorkerId, batch: LogBatch) {
        for entry in batch.entries {
            let line = LogLine {
                task_run_id: entry.task_run_id,
                timestamp_ms: entry.timestamp_ms,
                level: log_level_to_string(entry.level),
                message: entry.message,
                metadata: if entry.metadata.is_empty() {
                    None
                } else {
                    serde_json::from_str(&entry.metadata).ok()
                },
            };
            let _ = self.log_tx.send(line).await;
        }
    }

    /// Cancel a task on the worker that's running it
    pub async fn cancel_task_on_worker(&self, task_id: &str) -> bool {
        for entry in self.workers.iter() {
            let handle = entry.value();
            if handle.active_tasks.contains(task_id) {
                let cancel = WorkerResponse {
                    response: Some(worker_response::Response::TaskCancellation(
                        TaskCancellation {
                            task_id: task_id.to_string(),
                            reason: "Cancelled by user".to_string(),
                        },
                    )),
                };
                let _ = handle.response_tx.send(cancel).await;
                return true;
            }
        }
        false
    }

    /// Send a signal to the worker currently running a task. Returns true if delivered.
    pub async fn send_signal_to_worker(&self, task_id: &str, signal: TaskSignal) -> bool {
        for entry in self.workers.iter() {
            let handle = entry.value();
            if handle.active_tasks.contains(task_id) {
                let response = WorkerResponse {
                    response: Some(worker_response::Response::TaskSignal(signal)),
                };
                let _ = handle.response_tx.send(response).await;
                return true;
            }
        }
        false
    }

    /// Handle a signal acknowledgement from a worker
    pub async fn handle_signal_ack(&self, ack: &SignalAck) {
        self.router().signal_acked(&ack.signal_id).await;
    }

    pub fn workers(&self) -> &Arc<DashMap<String, WorkerHandle>> {
        &self.workers
    }

    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    /// Start the heartbeat checker background task
    pub fn start_heartbeat_checker(
        &self,
        shutdown: watch::Receiver<bool>,
    ) -> (tokio::task::JoinHandle<()>, mpsc::Receiver<WorkerId>) {
        let (dead_tx, dead_rx) = mpsc::channel(64);
        let workers = self.workers.clone();
        let handle = tokio::spawn(heartbeat::heartbeat_checker(workers, shutdown, dead_tx));
        (handle, dead_rx)
    }
}

pub fn log_level_to_string(level: i32) -> String {
    match level {
        1 => "DEBUG",
        2 => "INFO",
        3 => "WARN",
        4 => "ERROR",
        _ => "INFO",
    }
    .to_string()
}
