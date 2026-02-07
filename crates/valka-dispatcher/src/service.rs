use crate::heartbeat;
use crate::worker_handle::WorkerHandle;
use chrono::{Duration, Utc};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, error, info, warn};
use valka_core::{NodeId, PartitionId, TaskRunId, WorkerId};
use valka_db::DbPool;
use valka_matching::MatchingService;
use valka_matching::partition::TaskEnvelope;
use valka_proto::{
    Heartbeat, LogBatch, TaskAssignment, TaskResult, WorkerResponse, worker_response,
};

/// The dispatcher manages all connected workers and their gRPC streams.
#[derive(Clone)]
pub struct DispatcherService {
    workers: Arc<DashMap<String, WorkerHandle>>,
    matching: MatchingService,
    pool: DbPool,
    node_id: NodeId,
    _event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    log_tx: mpsc::Sender<valka_proto::LogEntry>,
}

impl DispatcherService {
    pub fn new(
        matching: MatchingService,
        pool: DbPool,
        node_id: NodeId,
        event_tx: broadcast::Sender<valka_proto::TaskEvent>,
        log_tx: mpsc::Sender<valka_proto::LogEntry>,
    ) -> Self {
        Self {
            workers: Arc::new(DashMap::new()),
            matching,
            pool,
            node_id,
            _event_tx: event_tx,
            log_tx,
        }
    }

    pub async fn register_worker(&self, handle: WorkerHandle) {
        let worker_id = handle.worker_id.clone();
        self.workers.insert(worker_id.0.clone(), handle);
        valka_core::metrics::set_active_workers(self.workers.len() as f64);
    }

    pub async fn deregister_worker(&self, worker_id: &WorkerId) {
        if let Some((_, handle)) = self.workers.remove(worker_id.as_ref()) {
            // Deregister from matching service
            self.matching.deregister_worker(worker_id);
            info!(
                worker_id = %worker_id,
                active_tasks = handle.active_tasks.len(),
                "Worker deregistered"
            );
            // Active tasks will be handled by lease expiry in the scheduler
        }
        valka_core::metrics::set_active_workers(self.workers.len() as f64);
    }

    /// Background loop: register as waiting in matching service, receive tasks, push to worker
    pub async fn run_worker_match_loop(&self, worker_id: WorkerId, queues: Vec<String>) {
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

            // Register as waiting on each queue
            for queue in &queues {
                let partition_id = PartitionId(0); // TODO: proper partition assignment
                let rx = self
                    .matching
                    .register_worker(queue, partition_id, worker_id.clone());

                // Wait for a match
                match rx.await {
                    Ok(envelope) => {
                        self.dispatch_to_worker(&worker_id, envelope).await;
                    }
                    Err(_) => {
                        // Channel closed, matching service reset
                        debug!(worker_id = %worker_id, "Match channel closed");
                    }
                }
            }
        }
    }

    async fn dispatch_to_worker(&self, worker_id: &WorkerId, mut envelope: TaskEnvelope) {
        // Create a task run
        let run_id = TaskRunId::new();
        envelope.task_run_id = run_id.0.clone();

        let lease_duration = Duration::seconds(envelope.timeout_seconds as i64 + 30);
        let lease_expires = Utc::now() + lease_duration;

        // Update task status to RUNNING and create task_run
        if let Err(e) =
            valka_db::queries::tasks::update_task_status(&self.pool, &envelope.task_id, "RUNNING")
                .await
        {
            error!(task_id = %envelope.task_id, error = %e, "Failed to update task status");
            return;
        }

        if let Err(e) = valka_db::queries::task_runs::create_task_run(
            &self.pool,
            valka_db::queries::task_runs::CreateTaskRunParams {
                id: run_id.0.clone(),
                task_id: envelope.task_id.clone(),
                attempt_number: envelope.attempt_number,
                worker_id: worker_id.0.clone(),
                assigned_node_id: self.node_id.0.clone(),
                lease_expires_at: lease_expires,
            },
        )
        .await
        {
            error!(task_id = %envelope.task_id, error = %e, "Failed to create task run");
            return;
        }

        // Build assignment message
        let assignment = TaskAssignment {
            task_id: envelope.task_id.clone(),
            task_run_id: run_id.0.clone(),
            queue_name: envelope.queue_name.clone(),
            task_name: envelope.task_name.clone(),
            input: envelope.input.unwrap_or_default(),
            attempt_number: envelope.attempt_number,
            timeout_seconds: envelope.timeout_seconds,
            metadata: envelope.metadata,
        };

        // Send to worker via their response channel
        if let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) {
            handle.assign_task(envelope.task_id.clone());
            let response = WorkerResponse {
                response: Some(worker_response::Response::TaskAssignment(assignment)),
            };
            if handle.response_tx.send(response).await.is_err() {
                warn!(worker_id = %worker_id, "Failed to send task assignment - worker disconnected");
            }
        }
    }

    pub async fn handle_task_result(&self, worker_id: &WorkerId, result: TaskResult) {
        // Update worker state
        if let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) {
            handle.complete_task(&result.task_id);
        }

        if result.success {
            // Complete the task run
            let output = if result.output.is_empty() {
                None
            } else {
                serde_json::from_str(&result.output).ok()
            };

            if let Err(e) = valka_db::queries::task_runs::complete_task_run(
                &self.pool,
                &result.task_run_id,
                output,
            )
            .await
            {
                error!(task_run_id = %result.task_run_id, error = %e, "Failed to complete task run");
            }

            if let Err(e) = valka_db::queries::tasks::update_task_status(
                &self.pool,
                &result.task_id,
                "COMPLETED",
            )
            .await
            {
                error!(task_id = %result.task_id, error = %e, "Failed to update task to COMPLETED");
            }

            valka_core::metrics::record_task_completed("");
        } else {
            // Fail the task run
            if let Err(e) = valka_db::queries::task_runs::fail_task_run(
                &self.pool,
                &result.task_run_id,
                &result.error_message,
            )
            .await
            {
                error!(task_run_id = %result.task_run_id, error = %e, "Failed to fail task run");
            }

            if result.retryable {
                // Mark for retry - scheduler will handle re-dispatch
                if let Err(e) = valka_db::queries::tasks::update_task_status(
                    &self.pool,
                    &result.task_id,
                    "RETRY",
                )
                .await
                {
                    error!(task_id = %result.task_id, error = %e, "Failed to update task to RETRY");
                }
                valka_core::metrics::record_task_retried("");
            } else {
                if let Err(e) = valka_db::queries::tasks::update_task_status(
                    &self.pool,
                    &result.task_id,
                    "FAILED",
                )
                .await
                {
                    error!(task_id = %result.task_id, error = %e, "Failed to update task to FAILED");
                }
                valka_core::metrics::record_task_failed("");
            }
        }
    }

    pub async fn handle_heartbeat(&self, worker_id: &WorkerId, _heartbeat: Heartbeat) {
        if let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) {
            handle.update_heartbeat();
        }
    }

    pub async fn handle_log_batch(&self, _worker_id: &WorkerId, batch: LogBatch) {
        for entry in batch.entries {
            let _ = self.log_tx.send(entry).await;
        }
    }

    pub fn workers(&self) -> &Arc<DashMap<String, WorkerHandle>> {
        &self.workers
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
