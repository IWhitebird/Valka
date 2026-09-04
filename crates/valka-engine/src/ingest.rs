//! Task log ingestion: buffer per run, flush chunks to the bucket.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};
use valka_wal::Store;
use valka_wal::logstore::{self, LogLine};

pub struct LogIngester {
    store: Store,
    batch_size: usize,
    flush_interval: Duration,
}

impl LogIngester {
    pub fn new(store: Store, batch_size: usize, flush_interval: Duration) -> Arc<Self> {
        Arc::new(Self {
            store,
            batch_size,
            flush_interval,
        })
    }

    pub async fn run(
        self: Arc<Self>,
        mut rx: mpsc::Receiver<LogLine>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut buffers: HashMap<String, Vec<LogLine>> = HashMap::new();
        let mut total = 0usize;
        let mut tick = tokio::time::interval(self.flush_interval);
        info!("Log ingester started");
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        self.flush_all(&mut buffers).await;
                        info!("Log ingester shutting down");
                        return;
                    }
                }
                item = rx.recv() => {
                    match item {
                        Some(line) => {
                            buffers.entry(line.task_run_id.clone()).or_default().push(line);
                            total += 1;
                            if total >= self.batch_size {
                                self.flush_all(&mut buffers).await;
                                total = 0;
                            }
                        }
                        None => {
                            self.flush_all(&mut buffers).await;
                            return;
                        }
                    }
                }
                _ = tick.tick() => {
                    if total > 0 {
                        self.flush_all(&mut buffers).await;
                        total = 0;
                    }
                }
            }
        }
    }

    async fn flush_all(&self, buffers: &mut HashMap<String, Vec<LogLine>>) {
        for (run_id, lines) in buffers.drain() {
            if let Err(e) = logstore::append_chunk(&self.store, &run_id, &lines).await {
                warn!(run_id, error = %e, count = lines.len(), "failed to write log chunk; dropping");
            } else {
                debug!(run_id, count = lines.len(), "flushed log chunk");
            }
        }
    }

    pub async fn read(&self, run_id: &str, limit: usize) -> Vec<LogLine> {
        logstore::read_run(&self.store, run_id, limit)
            .await
            .unwrap_or_default()
    }
}
