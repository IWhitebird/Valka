use chrono::{Duration, Utc};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::time;
use tracing::{info, warn};
use valka_core::WorkerId;

use crate::worker_handle::WorkerHandle;

const HEARTBEAT_TIMEOUT_SECS: i64 = 30;
const SUSPECT_AFTER_SECS: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerStatus {
    Alive,
    Suspect,
    Dead,
}

/// Check worker heartbeat status
pub fn check_heartbeat(handle: &WorkerHandle) -> WorkerStatus {
    let now = Utc::now();
    let elapsed = now - handle.last_heartbeat;

    if elapsed > Duration::seconds(HEARTBEAT_TIMEOUT_SECS) {
        WorkerStatus::Dead
    } else if elapsed > Duration::seconds(SUSPECT_AFTER_SECS) {
        WorkerStatus::Suspect
    } else {
        WorkerStatus::Alive
    }
}

/// Background task that periodically checks heartbeats and removes dead workers
pub async fn heartbeat_checker(
    workers: Arc<DashMap<String, WorkerHandle>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    on_worker_dead: tokio::sync::mpsc::Sender<WorkerId>,
) {
    let mut interval = time::interval(time::Duration::from_secs(5));

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("Heartbeat checker shutting down");
                    break;
                }
            }
            _ = interval.tick() => {
                let mut dead_workers = Vec::new();

                for entry in workers.iter() {
                    let status = check_heartbeat(entry.value());
                    match status {
                        WorkerStatus::Dead => {
                            warn!(
                                worker_id = %entry.value().worker_id,
                                worker_name = %entry.value().worker_name,
                                "Worker heartbeat timeout - marking as dead"
                            );
                            dead_workers.push(entry.key().clone());
                        }
                        WorkerStatus::Suspect => {
                            warn!(
                                worker_id = %entry.value().worker_id,
                                worker_name = %entry.value().worker_name,
                                "Worker heartbeat suspect"
                            );
                        }
                        WorkerStatus::Alive => {}
                    }
                }

                for worker_key in dead_workers {
                    if let Some((_, handle)) = workers.remove(&worker_key) {
                        let _ = on_worker_dead.send(handle.worker_id).await;
                    }
                }
            }
        }
    }
}
