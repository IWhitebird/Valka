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

/// A connection whose worker stopped heartbeating. The checker has already closed it;
/// whoever receives this deregisters that connection (and only that one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadWorker {
    pub worker_id: WorkerId,
    pub conn_id: u64,
}

/// Background task that periodically checks heartbeats. A worker silent for too long has
/// its connection closed (its stream ends, so a worker that is in fact alive reconnects)
/// and is reported on `on_worker_dead`. Its handle stays in `workers`: removing it here
/// would leave its stream open with no handle (a worker that gets nothing and is never
/// told) and skip the cleanup that resets its delivered signals.
pub async fn heartbeat_checker(
    workers: Arc<DashMap<String, WorkerHandle>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    on_worker_dead: tokio::sync::mpsc::Sender<DeadWorker>,
) {
    let mut interval = time::interval(time::Duration::from_secs(5));

    loop {
        tokio::select! {
            r = shutdown.changed() => {
                if r.is_err() || *shutdown.borrow() {
                    info!("Heartbeat checker shutting down");
                    break;
                }
            }
            _ = interval.tick() => {
                let mut dead_workers = Vec::new();

                for entry in workers.iter() {
                    let handle = entry.value();
                    match check_heartbeat(handle) {
                        WorkerStatus::Dead => {
                            warn!(
                                worker_id = %handle.worker_id,
                                worker_name = %handle.worker_name,
                                "Worker heartbeat timeout - closing its connection"
                            );
                            handle.close();
                            dead_workers.push(DeadWorker {
                                worker_id: handle.worker_id.clone(),
                                conn_id: handle.conn_id,
                            });
                        }
                        WorkerStatus::Suspect => {
                            warn!(
                                worker_id = %handle.worker_id,
                                worker_name = %handle.worker_name,
                                "Worker heartbeat suspect"
                            );
                        }
                        WorkerStatus::Alive => {}
                    }
                }

                for dead in dead_workers {
                    let _ = on_worker_dead.send(dead).await;
                }
            }
        }
    }
}
