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
    Heartbeat, LogBatch, SignalAck, TaskAssignment, TaskCancellation, TaskEvent, TaskResult,
    TaskSignal, WorkerResponse, worker_response,
};

/// The dispatcher manages all connected workers and their gRPC streams.
#[derive(Clone)]
pub struct DispatcherService {
    workers: Arc<DashMap<String, WorkerHandle>>,
    matching: MatchingService,
    pool: DbPool,
    node_id: NodeId,
    event_tx: broadcast::Sender<TaskEvent>,
    log_tx: mpsc::Sender<valka_proto::LogEntry>,
}

impl DispatcherService {
    pub fn new(
        matching: MatchingService,
        pool: DbPool,
        node_id: NodeId,
        event_tx: broadcast::Sender<TaskEvent>,
        log_tx: mpsc::Sender<valka_proto::LogEntry>,
    ) -> Self {
        Self {
            workers: Arc::new(DashMap::new()),
            matching,
            pool,
            node_id,
            event_tx,
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

            // Reset delivered (unacknowledged) signals for all active tasks
            for task_id in &handle.active_tasks {
                if let Err(e) =
                    valka_db::queries::signals::reset_delivered_signals(&self.pool, task_id).await
                {
                    warn!(task_id = %task_id, error = %e, "Failed to reset signals on deregister");
                }
            }

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
                    let rx = self.matching.register_worker(
                        queue,
                        partition_id,
                        worker_id.clone(),
                    );
                    receivers.push((queue.clone(), partition_id, rx));
                }
            }

            if receivers.is_empty() {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                continue;
            }

            let futs: Vec<_> = receivers
                .into_iter()
                .map(|(queue, pid, rx)| {
                    Box::pin(async move { (queue, pid, rx.await) })
                })
                .collect();

            let (first_result, _index, remaining) =
                futures::future::select_all(futs).await;

            for fut in remaining {
                if let Some((q, p, Ok(envelope))) = fut.now_or_never() {
                    self.matching.buffer_task(&q, p, envelope);
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

    async fn dispatch_to_worker(&self, worker_id: &WorkerId, mut envelope: TaskEnvelope) {
        // Create a task run
        let run_id = TaskRunId::new();
        envelope.task_run_id = run_id.0.clone();

        let lease_duration = Duration::seconds(envelope.timeout_seconds as i64 + 30);
        let lease_expires = Utc::now() + lease_duration;

        // Use a transaction to atomically: increment attempt, set RUNNING, create run
        let mut tx = match self.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                error!(task_id = %envelope.task_id, error = %e, "Failed to begin transaction");
                return;
            }
        };

        // Increment attempt count
        if let Err(e) = sqlx::query(
            "UPDATE tasks SET attempt_count = attempt_count + 1, updated_at = NOW() WHERE id = $1",
        )
        .bind(&envelope.task_id)
        .execute(&mut *tx)
        .await
        {
            error!(task_id = %envelope.task_id, error = %e, "Failed to increment attempt count");
            let _ = tx.rollback().await;
            return;
        }

        // Update task status to RUNNING
        if let Err(e) =
            sqlx::query("UPDATE tasks SET status = 'RUNNING', updated_at = NOW() WHERE id = $1")
                .bind(&envelope.task_id)
                .execute(&mut *tx)
                .await
        {
            error!(task_id = %envelope.task_id, error = %e, "Failed to update task status");
            let _ = tx.rollback().await;
            return;
        }

        // Create task run
        if let Err(e) = sqlx::query(
            r#"INSERT INTO task_runs (id, task_id, attempt_number, worker_id, assigned_node_id, lease_expires_at)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
        )
        .bind(&run_id.0)
        .bind(&envelope.task_id)
        .bind(envelope.attempt_number)
        .bind(&worker_id.0)
        .bind(&self.node_id.0)
        .bind(lease_expires)
        .execute(&mut *tx)
        .await
        {
            error!(task_id = %envelope.task_id, error = %e, "Failed to create task run");
            let _ = tx.rollback().await;
            return;
        }

        if let Err(e) = tx.commit().await {
            error!(task_id = %envelope.task_id, error = %e, "Failed to commit dispatch transaction");
            return;
        }

        // Record dispatch latency metric
        valka_core::metrics::record_dispatch_latency(&envelope.queue_name, 0.0);

        // Emit TaskEvent for RUNNING
        self.emit_event(&envelope.task_id, &envelope.queue_name, 3); // 3 = RUNNING

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
                return;
            }

            // Deliver any pending signals for this task
            let tx = handle.response_tx.clone();
            drop(handle); // Release DashMap guard before DB call
            match valka_db::queries::signals::get_pending_signals(&self.pool, &envelope.task_id)
                .await
            {
                Ok(signals) => {
                    for sig in signals {
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
                            let _ = valka_db::queries::signals::mark_delivered(&self.pool, &sig.id)
                                .await;
                        }
                    }
                }
                Err(e) => {
                    warn!(task_id = %envelope.task_id, error = %e, "Failed to load pending signals");
                }
            }
        }
    }

    pub async fn handle_task_result(&self, worker_id: &WorkerId, result: TaskResult) {
        // Update worker state
        if let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) {
            handle.complete_task(&result.task_id);
        }

        if result.success {
            let output: Option<serde_json::Value> = if result.output.is_empty() {
                None
            } else {
                serde_json::from_str(&result.output).ok()
            };

            // Atomically complete both run and task in a single transaction
            let tx_result: Result<(), sqlx::Error> = async {
                let mut tx = self.pool.begin().await?;

                sqlx::query(
                    "UPDATE task_runs SET status = 'COMPLETED', output = $2, completed_at = NOW() \
                     WHERE id = $1 AND status = 'RUNNING'",
                )
                .bind(&result.task_run_id)
                .bind(&output)
                .execute(&mut *tx)
                .await?;

                sqlx::query(
                    "UPDATE tasks SET status = 'COMPLETED', output = $2, updated_at = NOW() \
                     WHERE id = $1",
                )
                .bind(&result.task_id)
                .bind(&output)
                .execute(&mut *tx)
                .await?;

                tx.commit().await?;
                Ok(())
            }
            .await;

            if let Err(e) = tx_result {
                error!(
                    task_id = %result.task_id,
                    task_run_id = %result.task_run_id,
                    error = %e,
                    "Failed to complete task/run transaction"
                );
            }

            valka_core::metrics::record_task_completed("");
            self.emit_event(&result.task_id, "", 4); // 4 = COMPLETED
        } else {
            // Atomically fail run and update task status in a single transaction
            let tx_result: Result<(), sqlx::Error> = async {
                let mut tx = self.pool.begin().await?;

                sqlx::query(
                    "UPDATE task_runs SET status = 'FAILED', error_message = $2, \
                     completed_at = NOW() WHERE id = $1 AND status = 'RUNNING'",
                )
                .bind(&result.task_run_id)
                .bind(&result.error_message)
                .execute(&mut *tx)
                .await?;

                if result.retryable {
                    sqlx::query(
                        "UPDATE tasks SET status = 'RETRY', updated_at = NOW() WHERE id = $1",
                    )
                    .bind(&result.task_id)
                    .execute(&mut *tx)
                    .await?;
                } else {
                    sqlx::query(
                        "UPDATE tasks SET status = 'FAILED', error_message = $2, \
                         updated_at = NOW() WHERE id = $1",
                    )
                    .bind(&result.task_id)
                    .bind(&result.error_message)
                    .execute(&mut *tx)
                    .await?;
                }

                tx.commit().await?;
                Ok(())
            }
            .await;

            if let Err(e) = tx_result {
                error!(
                    task_id = %result.task_id,
                    task_run_id = %result.task_run_id,
                    error = %e,
                    "Failed to process task result transaction"
                );
            }

            if result.retryable {
                valka_core::metrics::record_task_retried("");
                self.emit_event(&result.task_id, "", 6); // 6 = RETRY
            } else {
                valka_core::metrics::record_task_failed("");
                self.emit_event(&result.task_id, "", 5); // 5 = FAILED
            }
        }
    }

    pub async fn handle_heartbeat(&self, worker_id: &WorkerId, heartbeat: Heartbeat) {
        if let Some(mut handle) = self.workers.get_mut(worker_id.as_ref()) {
            handle.update_heartbeat();

            // Extend leases for active tasks
            for task_id in &heartbeat.active_task_ids {
                // Look up the task run ID from active tasks
                // We use the task_id to update the lease on any RUNNING run
                let lease_extension = Duration::seconds(60); // Extend by 60 seconds
                let new_lease = Utc::now() + lease_extension;
                // Update heartbeat for all running runs of this task
                if let Err(e) = valka_db::queries::task_runs::update_heartbeat_by_task(
                    &self.pool, task_id, new_lease,
                )
                .await
                {
                    error!(task_id = %task_id, error = %e, "Failed to extend task run lease");
                }
            }
        }
    }

    pub async fn handle_log_batch(&self, _worker_id: &WorkerId, batch: LogBatch) {
        for entry in batch.entries {
            let _ = self.log_tx.send(entry).await;
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
        if let Err(e) =
            valka_db::queries::signals::mark_acknowledged(&self.pool, &ack.signal_id).await
        {
            warn!(signal_id = %ack.signal_id, error = %e, "Failed to acknowledge signal");
        }
    }

    pub fn workers(&self) -> &Arc<DashMap<String, WorkerHandle>> {
        &self.workers
    }

    pub fn event_tx(&self) -> &broadcast::Sender<TaskEvent> {
        &self.event_tx
    }

    /// Emit a task event
    fn emit_event(&self, task_id: &str, queue_name: &str, new_status: i32) {
        let event = TaskEvent {
            event_id: uuid::Uuid::now_v7().to_string(),
            task_id: task_id.to_string(),
            queue_name: queue_name.to_string(),
            previous_status: 0,
            new_status,
            worker_id: String::new(),
            node_id: self.node_id.0.clone(),
            attempt_number: 0,
            error_message: String::new(),
            timestamp_ms: Utc::now().timestamp_millis(),
        };
        let _ = self.event_tx.send(event);
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
