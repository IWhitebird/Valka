use crate::heartbeat;
use crate::worker_handle::WorkerHandle;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};
use valka_core::{NodeId, PartitionId, WorkerId};
use valka_engine::Engine;
use valka_matching::MatchingService;
use valka_matching::partition::TaskEnvelope;
use valka_proto::{
    Heartbeat, LogBatch, SignalAck, StepCheckpoint, TaskAssignment, TaskCancellation, TaskResult,
    TaskSignal, WorkerResponse, worker_response,
};
use valka_wal::logstore::LogLine;

/// The dispatcher manages all connected workers and their gRPC streams.
#[derive(Clone)]
pub struct DispatcherService {
    workers: Arc<DashMap<String, WorkerHandle>>,
    matching: MatchingService,
    engine: Engine,
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
            engine,
            node_id,
            log_tx,
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
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
            for task_id in &handle.active_tasks {
                self.engine.reset_signals(task_id);
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

    /// Background loop: register as waiting in matching service, receive tasks, push to worker
    pub async fn run_worker_match_loop(&self, worker_id: WorkerId, queues: Vec<String>) {
        use futures::FutureExt;

        let num_partitions = self.matching.config().num_partitions;

        loop {
            let available = {
                match self.workers.get(worker_id.as_ref()) {
                    Some(handle) => handle.available_slots(),
                    None => return, // Worker disconnected
                }
            };

            if available <= 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                continue;
            }

            let mut receivers = Vec::new();
            for queue in &queues {
                for pid in 0..num_partitions {
                    let partition_id = PartitionId(pid);
                    let rx = self
                        .matching
                        .register_worker(queue, partition_id, worker_id.clone());
                    receivers.push((queue.clone(), partition_id, rx));
                }
            }

            if receivers.is_empty() {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                continue;
            }

            let futs: Vec<_> = receivers
                .into_iter()
                .map(|(queue, pid, rx)| Box::pin(async move { (queue, pid, rx.await) }))
                .collect();

            let (first_result, _index, remaining) = futures::future::select_all(futs).await;

            for fut in remaining {
                if let Some((q, p, Ok(envelope))) = fut.now_or_never()
                    && !self
                        .matching
                        .buffer_task(&q, p, envelope.clone_for_requeue())
                {
                    self.engine.unoffer(&envelope.task_id);
                }
            }

            match first_result.2 {
                Ok(envelope) => {
                    self.dispatch_to_worker(&worker_id, envelope).await;
                }
                Err(_) => {
                    debug!(worker_id = %worker_id, "Match channel closed");
                }
            }
        }
    }

    async fn dispatch_to_worker(&self, worker_id: &WorkerId, envelope: TaskEnvelope) {
        // Is the worker still here? If not, hand the task back before recording anything.
        if !self.workers.contains_key(worker_id.as_ref()) {
            self.engine.unoffer(&envelope.task_id);
            return;
        }

        // Record the dispatch (RUNNING + run + lease) in the engine.
        let info = match self.engine.dispatch(&envelope.task_id, &worker_id.0) {
            Ok(info) => info,
            Err(e) => {
                // Cancelled / deleted / already running: nothing to do.
                debug!(task_id = %envelope.task_id, error = %e, "dispatch refused by engine");
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
                // Worker vanished between the check and now: the lease will expire and
                // the task retries. At-least-once.
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
            return;
        }

        // Deliver any pending signals for this task.
        for sig in self.engine.pending_signals(&envelope.task_id) {
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
                self.engine.signal_delivered(&sig.id);
            }
        }
    }

    pub async fn handle_task_result(&self, worker_id: &WorkerId, result: TaskResult) {
        if let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) {
            handle.complete_task(&result.task_id);
        }

        let outcome = if result.success {
            let output: Option<serde_json::Value> = if result.output.is_empty() {
                None
            } else {
                serde_json::from_str(&result.output).ok()
            };
            self.engine
                .complete_run(&result.task_id, &result.task_run_id, output)
                .await
                .map(|_| ())
        } else {
            self.engine
                .fail_run(
                    &result.task_id,
                    &result.task_run_id,
                    &result.error_message,
                    result.retryable,
                )
                .await
                .map(|_| ())
        };

        if let Err(e) = outcome {
            // Stale result (task cancelled, lease already expired, duplicate): log and drop.
            warn!(
                task_id = %result.task_id,
                task_run_id = %result.task_run_id,
                error = %e,
                "task result not applied"
            );
        }
    }

    pub async fn handle_heartbeat(&self, worker_id: &WorkerId, heartbeat: Heartbeat) {
        if let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) {
            handle.update_heartbeat();
        }
        self.engine.heartbeat(&heartbeat.active_task_ids);
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
        self.engine.signal_acked(&ack.signal_id);
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
