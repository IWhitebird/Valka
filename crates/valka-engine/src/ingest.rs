//! Task log ingestion: buffer per run, flush chunks to the bucket.
//!
//! The receive loop never waits on the bucket: a flush runs in its own task while lines
//! keep arriving, and its chunk PUTs (one per run) run concurrently. Senders (the worker
//! streams) block on a full channel, and a blocked worker stream stops reading heartbeats
//! and results, so draining the channel must not depend on how fast the bucket is.
//! Flushes run one at a time, so a run's chunks are written in the order of its lines.

use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};
use valka_wal::Store;
use valka_wal::logstore::{self, LogLine};

/// Chunk PUTs in flight at once within a flush.
const PUT_CONCURRENCY: usize = 32;
/// Lines buffered while a flush is in flight. Past it, new lines are dropped (and
/// counted): logs are best effort, and blocking the channel would stall the workers.
const MAX_BUFFERED_LINES: usize = 100_000;

type Buffers = HashMap<String, Vec<LogLine>>;

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
        let mut buffers: Buffers = HashMap::new();
        let mut total = 0usize;
        let mut dropped = 0u64;
        let mut flushing: JoinSet<()> = JoinSet::new();
        let mut tick = tokio::time::interval(self.flush_interval);
        info!("Log ingester started");
        loop {
            tokio::select! {
                r = shutdown.changed() => {
                    if r.is_err() || *shutdown.borrow() {
                        info!("Log ingester shutting down");
                        // Keep what is already queued.
                        rx.close();
                        while let Ok(line) = rx.try_recv() {
                            if total < MAX_BUFFERED_LINES {
                                buffers.entry(line.task_run_id.clone()).or_default().push(line);
                                total += 1;
                            }
                        }
                        break;
                    }
                }
                item = rx.recv() => {
                    let Some(line) = item else { break };
                    if total >= MAX_BUFFERED_LINES {
                        dropped += 1;
                        if dropped == 1 || dropped.is_multiple_of(10_000) {
                            warn!(dropped, "log ingester behind the bucket; dropping log lines");
                        }
                        continue;
                    }
                    buffers.entry(line.task_run_id.clone()).or_default().push(line);
                    total += 1;
                    if total >= self.batch_size && flushing.is_empty() {
                        self.start_flush(&mut flushing, &mut buffers, &mut total);
                    }
                }
                _ = tick.tick() => {
                    if total > 0 && flushing.is_empty() {
                        self.start_flush(&mut flushing, &mut buffers, &mut total);
                    }
                }
                Some(_) = flushing.join_next(), if !flushing.is_empty() => {
                    if total >= self.batch_size {
                        self.start_flush(&mut flushing, &mut buffers, &mut total);
                    }
                }
            }
        }
        while flushing.join_next().await.is_some() {}
        self.flush(std::mem::take(&mut buffers)).await;
    }

    fn start_flush(&self, flushing: &mut JoinSet<()>, buffers: &mut Buffers, total: &mut usize) {
        let batch = std::mem::take(buffers);
        *total = 0;
        let me = Self {
            store: self.store.clone(),
            batch_size: self.batch_size,
            flush_interval: self.flush_interval,
        };
        flushing.spawn(async move { me.flush(batch).await });
    }

    async fn flush(&self, batch: Buffers) {
        futures::stream::iter(batch)
            .map(|(run_id, lines)| async move {
                match logstore::append_chunk(&self.store, &run_id, &lines).await {
                    Ok(()) => debug!(run_id, count = lines.len(), "flushed log chunk"),
                    Err(e) => {
                        warn!(run_id, error = %e, count = lines.len(), "failed to write log chunk; dropping")
                    }
                }
            })
            .buffer_unordered(PUT_CONCURRENCY)
            .collect::<()>()
            .await;
    }

    pub async fn read(&self, run_id: &str, limit: usize) -> Vec<LogLine> {
        logstore::read_run(&self.store, run_id, limit)
            .await
            .unwrap_or_default()
    }
}
