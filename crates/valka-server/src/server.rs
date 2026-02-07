use sqlx::PgPool;
use tokio::sync::{mpsc, watch};
use tokio::time::{Duration, interval};
use tracing::{error, info};
use valka_core::{LogIngesterConfig, SchedulerConfig};
use valka_db::queries::task_logs::{InsertLogEntry, batch_insert_logs};

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
