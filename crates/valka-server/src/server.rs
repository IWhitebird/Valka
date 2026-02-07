use sqlx::PgPool;
use std::collections::HashSet;
use tokio::sync::{mpsc, watch};
use tokio::time::{Duration, interval};
use tracing::{error, info};
use valka_core::{LogIngesterConfig, MatchingConfig, PartitionId, SchedulerConfig};
use valka_db::queries::task_logs::{InsertLogEntry, batch_insert_logs};
use valka_matching::MatchingService;
use valka_matching::task_reader::TaskReader;

/// Run the scheduler loop (leader election + periodic tasks)
pub async fn run_scheduler(
    pool: PgPool,
    config: SchedulerConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut election = valka_scheduler::SchedulerElection::new(pool.clone());
    let mut reaper_interval = interval(Duration::from_secs(config.reaper_interval_secs));
    let mut retry_interval = interval(Duration::from_secs(config.reaper_interval_secs));
    let mut dlq_interval = interval(Duration::from_secs(config.dlq_check_interval_secs));
    let mut delayed_interval = interval(Duration::from_secs(config.delayed_check_interval_secs));

    info!("Scheduler started");

    loop {
        // Try to acquire leadership
        match election.try_acquire().await {
            Ok(true) => {}
            Ok(false) => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
            Err(e) => {
                error!(error = %e, "Scheduler election error");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }

        // Leader loop
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("Scheduler shutting down");
                        let _ = election.release().await;
                        return;
                    }
                }
                _ = reaper_interval.tick() => {
                    if let Err(e) = valka_scheduler::reaper::reap_expired_leases(&pool).await {
                        error!(error = %e, "Reaper error");
                    }
                }
                _ = retry_interval.tick() => {
                    if let Err(e) = valka_scheduler::retry::process_retries(
                        &pool,
                        config.retry_base_delay_secs,
                        config.retry_max_delay_secs,
                    ).await {
                        error!(error = %e, "Retry processor error");
                    }
                }
                _ = dlq_interval.tick() => {
                    if let Err(e) = valka_scheduler::dlq::process_dead_letters(&pool).await {
                        error!(error = %e, "DLQ processor error");
                    }
                }
                _ = delayed_interval.tick() => {
                    if let Err(e) = valka_scheduler::delayed::promote_delayed_tasks(&pool).await {
                        error!(error = %e, "Delayed task promoter error");
                    }
                }
            }
        }
    }
}

/// Run the log ingester: batch log entries from workers and flush to PG
pub async fn run_log_ingester(
    pool: PgPool,
    config: LogIngesterConfig,
    mut log_rx: mpsc::Receiver<valka_proto::LogEntry>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut buffer: Vec<InsertLogEntry> = Vec::with_capacity(config.batch_size);
    let mut flush_interval = interval(Duration::from_millis(config.flush_interval_ms));

    info!("Log ingester started");

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    // Flush remaining
                    if !buffer.is_empty() {
                        let _ = flush_logs(&pool, &mut buffer).await;
                    }
                    info!("Log ingester shutting down");
                    return;
                }
            }
            Some(entry) = log_rx.recv() => {
                buffer.push(InsertLogEntry {
                    task_run_id: entry.task_run_id,
                    timestamp_ms: entry.timestamp_ms,
                    level: log_level_to_string(entry.level),
                    message: entry.message,
                    metadata: if entry.metadata.is_empty() {
                        None
                    } else {
                        serde_json::from_str(&entry.metadata).ok()
                    },
                });

                if buffer.len() >= config.batch_size {
                    let _ = flush_logs(&pool, &mut buffer).await;
                }
            }
            _ = flush_interval.tick() => {
                if !buffer.is_empty() {
                    let _ = flush_logs(&pool, &mut buffer).await;
                }
            }
        }
    }
}

async fn flush_logs(pool: &PgPool, buffer: &mut Vec<InsertLogEntry>) -> Result<(), sqlx::Error> {
    let entries: Vec<InsertLogEntry> = std::mem::take(buffer);
    let count = entries.len();
    batch_insert_logs(pool, &entries).await?;
    tracing::debug!(count, "Flushed log entries to PG");
    Ok(())
}

fn log_level_to_string(level: i32) -> String {
    match level {
        1 => "DEBUG".to_string(),
        2 => "INFO".to_string(),
        3 => "WARN".to_string(),
        4 => "ERROR".to_string(),
        _ => "INFO".to_string(),
    }
}

/// Discover queues from PG and start TaskReaders for them
pub async fn run_task_reader_manager(
    pool: PgPool,
    matching: MatchingService,
    config: MatchingConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut known_queues: HashSet<String> = HashSet::new();
    let mut check_interval = interval(Duration::from_secs(5));

    info!("TaskReader manager started");

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("TaskReader manager shutting down");
                    return;
                }
            }
            _ = check_interval.tick() => {
                // Discover queues from the tasks table
                match discover_queues(&pool).await {
                    Ok(queues) => {
                        for queue_name in queues {
                            if known_queues.contains(&queue_name) {
                                continue;
                            }
                            known_queues.insert(queue_name.clone());

                            // Ensure queue partitions exist
                            matching.ensure_queue(&queue_name);

                            // Start a TaskReader for each partition
                            for pid in 0..config.num_partitions {
                                let reader = TaskReader::new(
                                    pool.clone(),
                                    matching.clone(),
                                    queue_name.clone(),
                                    PartitionId(pid),
                                    config.clone(),
                                    shutdown.clone(),
                                );
                                tokio::spawn(reader.run());
                            }

                            info!(queue = %queue_name, partitions = config.num_partitions, "Started TaskReaders for queue");
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to discover queues");
                    }
                }

                // Update pending tasks metrics
                if let Ok(counts) = valka_db::queries::tasks::count_pending_by_queue(&pool).await {
                    for (queue, count) in counts {
                        valka_core::metrics::set_pending_tasks(&queue, count as f64);
                    }
                }
            }
        }
    }
}

async fn discover_queues(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT DISTINCT queue_name FROM tasks")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(name,)| name).collect())
}
