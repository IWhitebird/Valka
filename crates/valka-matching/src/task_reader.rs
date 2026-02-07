use crate::partition::TaskEnvelope;
use crate::service::MatchingService;
use sqlx::PgPool;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};
use valka_core::{MatchingConfig, PartitionId};

/// Background loop that reads PENDING tasks from PG (SKIP LOCKED) and feeds them
/// into the matching service for async dispatch.
pub struct TaskReader {
    pool: PgPool,
    matching: MatchingService,
    queue_name: String,
    partition_id: PartitionId,
    config: MatchingConfig,
    shutdown: tokio::sync::watch::Receiver<bool>,
}

impl TaskReader {
    pub fn new(
        pool: PgPool,
        matching: MatchingService,
        queue_name: String,
        partition_id: PartitionId,
        config: MatchingConfig,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            pool,
            matching,
            queue_name,
            partition_id,
            config,
            shutdown,
        }
    }

    pub async fn run(mut self) {
        info!(
            queue = %self.queue_name,
            partition = self.partition_id.0,
            "TaskReader started"
        );

        let busy_interval = Duration::from_millis(self.config.task_reader_poll_busy_ms);
        let idle_interval = Duration::from_millis(self.config.task_reader_poll_idle_ms);
        let mut current_interval = idle_interval;

        loop {
            tokio::select! {
                _ = self.shutdown.changed() => {
                    if *self.shutdown.borrow() {
                        info!(
                            queue = %self.queue_name,
                            partition = self.partition_id.0,
                            "TaskReader shutting down"
                        );
                        break;
                    }
                }
                _ = sleep(current_interval) => {
                    match self.poll_and_dispatch().await {
                        Ok(count) if count > 0 => {
                            debug!(
                                queue = %self.queue_name,
                                partition = self.partition_id.0,
                                count,
                                "TaskReader dispatched tasks"
                            );
                            current_interval = busy_interval; // Tasks found, poll fast
                        }
                        Ok(_) => {
                            current_interval = idle_interval; // No tasks, slow down
                        }
                        Err(e) => {
                            error!(
                                queue = %self.queue_name,
                                partition = self.partition_id.0,
                                error = %e,
                                "TaskReader poll error"
                            );
                            current_interval = idle_interval;
                        }
                    }
                }
            }
        }
    }

    async fn poll_and_dispatch(&self) -> Result<usize, sqlx::Error> {
        let tasks = valka_db::queries::tasks::dequeue_tasks(
            &self.pool,
            &self.queue_name,
            self.partition_id.0,
            self.config.task_reader_batch_size,
        )
        .await?;

        let count = tasks.len();

        for task_row in tasks {
            let envelope = TaskEnvelope {
                task_id: task_row.id.clone(),
                task_run_id: String::new(), // Will be assigned by dispatcher
                queue_name: task_row.queue_name.clone(),
                task_name: task_row.task_name.clone(),
                input: task_row.input.map(|v| v.to_string()),
                attempt_number: task_row.attempt_count + 1,
                timeout_seconds: task_row.timeout_seconds,
                metadata: task_row.metadata.to_string(),
                priority: task_row.priority,
            };

            // Try sync match first
            match self
                .matching
                .offer_task(&self.queue_name, self.partition_id, envelope)
            {
                Ok(()) => {
                    valka_core::metrics::record_async_match();
                }
                Err(envelope) => {
                    // No worker available, buffer it
                    if !self
                        .matching
                        .buffer_task(&self.queue_name, self.partition_id, envelope)
                    {
                        // Buffer full, task stays DISPATCHING in PG
                        // The reaper will eventually reset it to PENDING
                        warn!(
                            queue = %self.queue_name,
                            partition = self.partition_id.0,
                            task_id = %task_row.id,
                            "Buffer full, task remains in DISPATCHING state"
                        );
                    }
                }
            }
        }

        Ok(count)
    }
}
