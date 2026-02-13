use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::{Duration, interval};
use tracing::{error, info};
use valka_cluster::ClusterManager;
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

/// Discover queues from PG and start TaskReaders for owned partitions.
/// When cluster membership changes (PartitionsRebalanced), reconciles readers:
/// stops readers for partitions we no longer own, starts readers for newly owned ones.
pub async fn run_task_reader_manager(
    pool: PgPool,
    matching: MatchingService,
    config: MatchingConfig,
    cluster: Arc<ClusterManager>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut known_queues: HashSet<String> = HashSet::new();
    // (queue_name, partition_id) -> shutdown sender for that reader
    let mut reader_shutdowns: HashMap<(String, i32), watch::Sender<bool>> = HashMap::new();
    let mut check_interval = interval(Duration::from_secs(5));
    let mut cluster_events = cluster.subscribe_events();

    info!("TaskReader manager started");

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    // Shut down all readers
                    for (_, tx) in reader_shutdowns.drain() {
                        let _ = tx.send(true);
                    }
                    info!("TaskReader manager shutting down");
                    return;
                }
            }
            event = cluster_events.recv() => {
                match event {
                    Ok(valka_cluster::ClusterEvent::PartitionsRebalanced) => {
                        // Reconcile readers on rebalance
                        reconcile_readers(
                            &pool,
                            &matching,
                            &config,
                            &cluster,
                            &known_queues,
                            &mut reader_shutdowns,
                        ).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(n, "TaskReader manager lagged on cluster events, reconciling");
                        reconcile_readers(
                            &pool,
                            &matching,
                            &config,
                            &cluster,
                            &known_queues,
                            &mut reader_shutdowns,
                        ).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Cluster event channel closed");
                    }
                    _ => {}
                }
            }
            _ = check_interval.tick() => {
                // Discover queues from the tasks table
                match discover_queues(&pool).await {
                    Ok(queues) => {
                        let mut new_queues = false;
                        for queue_name in queues {
                            if known_queues.contains(&queue_name) {
                                continue;
                            }
                            known_queues.insert(queue_name.clone());
                            new_queues = true;

                            // Ensure queue partitions exist
                            matching.ensure_queue(&queue_name);

                            // Start readers only for partitions we own
                            for pid in 0..config.num_partitions {
                                let key = (queue_name.clone(), pid);
                                if reader_shutdowns.contains_key(&key) {
                                    continue;
                                }
                                if !cluster.owns_partition(&queue_name, pid).await {
                                    continue;
                                }
                                start_reader(
                                    &pool,
                                    &matching,
                                    &config,
                                    &queue_name,
                                    pid,
                                    &mut reader_shutdowns,
                                );
                            }

                            info!(queue = %queue_name, "Started TaskReaders for owned partitions");
                        }

                        if !new_queues {
                            // Even if no new queues, periodically reconcile
                            // to handle ownership changes without explicit event
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

/// Reconcile readers: stop readers for partitions we no longer own,
/// start readers for partitions we now own.
async fn reconcile_readers(
    pool: &PgPool,
    matching: &MatchingService,
    config: &MatchingConfig,
    cluster: &Arc<ClusterManager>,
    known_queues: &HashSet<String>,
    reader_shutdowns: &mut HashMap<(String, i32), watch::Sender<bool>>,
) {
    // Stop readers for partitions we no longer own
    let keys_to_check: Vec<(String, i32)> = reader_shutdowns.keys().cloned().collect();
    for key in keys_to_check {
        if !cluster.owns_partition(&key.0, key.1).await
            && let Some(tx) = reader_shutdowns.remove(&key)
        {
            let _ = tx.send(true);
            info!(
                queue = %key.0,
                partition = key.1,
                "Stopped TaskReader (partition no longer owned)"
            );
        }
    }

    // Start readers for partitions we now own but don't have a reader for
    for queue_name in known_queues {
        matching.ensure_queue(queue_name);
        for pid in 0..config.num_partitions {
            let key = (queue_name.clone(), pid);
            if reader_shutdowns.contains_key(&key) {
                continue;
            }
            if !cluster.owns_partition(queue_name, pid).await {
                continue;
            }
            start_reader(pool, matching, config, queue_name, pid, reader_shutdowns);
            info!(
                queue = %queue_name,
                partition = pid,
                "Started TaskReader (newly owned partition)"
            );
        }
    }
}

fn start_reader(
    pool: &PgPool,
    matching: &MatchingService,
    config: &MatchingConfig,
    queue_name: &str,
    partition_id: i32,
    reader_shutdowns: &mut HashMap<(String, i32), watch::Sender<bool>>,
) {
    let (reader_shutdown_tx, reader_shutdown_rx) = watch::channel(false);
    let reader = TaskReader::new(
        pool.clone(),
        matching.clone(),
        queue_name.to_string(),
        PartitionId(partition_id),
        config.clone(),
        reader_shutdown_rx,
    );
    tokio::spawn(reader.run());
    reader_shutdowns.insert((queue_name.to_string(), partition_id), reader_shutdown_tx);
}

async fn discover_queues(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT queue_name FROM tasks")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|(name,)| name).collect())
}
