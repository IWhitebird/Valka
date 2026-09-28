//! Post-replay index rebuild and the snapshot / truncation round.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use valka_core::{ShardId, TaskStatus};
use valka_wal::{Lsn, reader, snapshot};

use crate::engine::Engine;
use crate::state::{RunStatus, ShardSnapshot, ShardState};
use crate::timers::TimerKind;
use crate::write_path::pending_key;

impl Engine {
    /// After recovery: pending index, timers, retention.
    pub(crate) fn rebuild_indexes(&self) {
        let now = self.inner.clock.now();
        let grace = ChronoDuration::seconds(self.inner.cfg.scheduler.recovery_grace_secs);
        let retention = ChronoDuration::seconds(self.inner.cfg.wal.completed_retention_secs as i64);
        let mut pending = self.inner.pending.lock();
        let mut timers = self.inner.timers.lock();
        let mut running = 0usize;
        let mut runnable = 0usize;
        let mut evicted = 0usize;
        for m in &self.inner.shards {
            let mut st = m.lock();
            evicted += st.evict_terminal_before(now - retention);
            for t in st.tasks.values_mut() {
                t.offered = false;
                match t.status {
                    TaskStatus::Pending if t.due => {
                        pending
                            .entry(t.spec.queue_name.clone())
                            .or_default()
                            .insert(pending_key(t));
                        runnable += 1;
                    }
                    TaskStatus::Pending | TaskStatus::Retry => {
                        let at = t.next_attempt_at.unwrap_or(now);
                        timers.schedule(
                            at,
                            TimerKind::Promote {
                                task_id: t.spec.id.clone(),
                            },
                        );
                    }
                    TaskStatus::Running => {
                        let task_id = t.spec.id.clone();
                        if let Some(run) = t.current_run_mut()
                            && run.status == RunStatus::Running
                        {
                            run.lease_until = run.lease_until.max(now + grace);
                            timers.schedule(
                                run.lease_until,
                                TimerKind::LeaseExpiry {
                                    task_id,
                                    run_id: run.id.clone(),
                                },
                            );
                            running += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        timers.schedule(now + ChronoDuration::seconds(60), TimerKind::Evict);
        info!(runnable, running, evicted, "indexes rebuilt");
        drop(pending);
        self.inner.pending_wake.notify_one();
    }

    pub(crate) async fn snapshotter(self) {
        let mut shutdown = self.inner.shutdown.subscribe();
        let interval = Duration::from_secs(self.inner.cfg.wal.snapshot_interval_secs.max(1));
        let tick = interval.min(Duration::from_secs(1));
        let mut last_round = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _ = shutdown.changed() => { if *shutdown.borrow() { return; } }
                _ = tokio::time::sleep(tick) => {}
            }
            if self.over_log_budget() {
                self.snapshot_dirty_shards(true).await;
                last_round = tokio::time::Instant::now();
            } else if last_round.elapsed() >= interval {
                self.snapshot_dirty_shards(false).await;
                last_round = tokio::time::Instant::now();
            }
        }
    }

    fn over_log_budget(&self) -> bool {
        let budget = self.inner.cfg.wal.log_budget_bytes;
        let since = self
            .inner
            .writer
            .bytes_committed()
            .saturating_sub(self.inner.log_mark.load(Ordering::SeqCst));
        budget > 0 && since >= budget
    }

    /// Snapshot every shard with uncovered records (or, when `force`, every dirty shard
    /// regardless of the record threshold), then prune snapshots and truncate segments.
    pub(crate) async fn snapshot_dirty_shards(&self, force: bool) {
        if force {
            self.inner
                .log_mark
                .store(self.inner.writer.bytes_committed(), Ordering::SeqCst);
        }
        let threshold = self.inner.cfg.wal.snapshot_after_records;
        let mut written = 0usize;
        let mut batch = Vec::with_capacity(SNAPSHOT_BATCH);
        for (i, m) in self.inner.shards.iter().enumerate() {
            {
                let mut st = m.lock();
                if st.records_since_snapshot == 0 {
                    continue;
                }
                if !force
                    && st.records_since_snapshot < threshold
                    && st
                        .dirty_since_lsn
                        .is_some_and(|d| d > self.inner.writer.durable_lsn())
                {
                    // Not enough records yet and nothing durable to cover: wait.
                    continue;
                }
                let lsn = self.inner.writer.next_lsn();
                let prev = Bookkeeping::of(&st);
                let taken_at = self.inner.clock.now();
                let snap = st.to_snapshot(taken_at);
                st.records_since_snapshot = 0;
                st.snapshot_seq = st.shard_seq;
                st.snapshot_lsn = lsn;
                st.dirty_since_lsn = None;
                st.snapshot_at = Some(taken_at);
                batch.push(Captured {
                    shard: ShardId(i as u16),
                    lsn,
                    snap,
                    prev,
                });
            }
            if batch.len() == SNAPSHOT_BATCH {
                written += self.write_snapshot_batch(std::mem::take(&mut batch)).await;
            }
        }
        if !batch.is_empty() {
            written += self.write_snapshot_batch(batch).await;
        }
        if written == 0 {
            return;
        }
        *self.inner.last_snapshot_round.lock() = Some(self.inner.clock.now());
        // Truncate: segments below every dirty shard's first uncovered record, and below
        // the durable watermark, are covered by snapshots.
        let durable = self.inner.writer.durable_lsn();
        let mut bound = Lsn::new(durable.epoch, durable.seq + 1);
        for m in &self.inner.shards {
            if let Some(d) = m.lock().dirty_since_lsn {
                bound = bound.min(d);
            }
        }
        match reader::truncate_before(&self.inner.store, &self.inner.cfg.node_id, bound).await {
            Ok(n) if n > 0 => {
                info!(snapshots = written, truncated = n, %bound, "snapshot round complete")
            }
            Ok(_) => debug!(snapshots = written, "snapshot round complete"),
            Err(e) => warn!(error = %e, "segment truncation failed"),
        }
    }

    /// Every record a captured state reflects was appended before its capture, so one sync
    /// after capturing covers them all. If that sync fails, nothing in the batch may be
    /// written: a snapshot would outlive records that never became durable.
    async fn write_snapshot_batch(&self, batch: Vec<Captured>) -> usize {
        if let Err(e) = self.inner.writer.sync().await {
            error!(error = %e, shards = batch.len(), "WAL not durable; snapshot batch aborted");
            for c in &batch {
                self.restore_bookkeeping(c);
            }
            return 0;
        }
        let mut written = 0;
        for c in batch {
            match snapshot::write(&self.inner.store, c.shard, c.lsn, &c.snap).await {
                Ok(()) => {
                    written += 1;
                    if let Err(e) = snapshot::prune(
                        &self.inner.store,
                        c.shard,
                        self.inner.cfg.wal.snapshots_to_keep.max(1),
                    )
                    .await
                    {
                        warn!(shard = %c.shard, error = %e, "snapshot prune failed");
                    }
                }
                Err(e) => {
                    error!(shard = %c.shard, error = %e, "snapshot write failed; keeping WAL");
                    self.restore_bookkeeping(&c);
                }
            }
        }
        written
    }

    fn restore_bookkeeping(&self, c: &Captured) {
        let mut st = self.inner.shards[c.shard.0 as usize].lock();
        st.records_since_snapshot += c.prev.records_since_snapshot;
        st.snapshot_seq = c.prev.snapshot_seq;
        st.snapshot_lsn = c.prev.snapshot_lsn;
        st.dirty_since_lsn = c.prev.dirty_since_lsn.or(Some(c.lsn));
        st.snapshot_at = c.prev.snapshot_at;
    }
}

const SNAPSHOT_BATCH: usize = 64;

struct Captured {
    shard: ShardId,
    lsn: Lsn,
    snap: ShardSnapshot,
    prev: Bookkeeping,
}

struct Bookkeeping {
    records_since_snapshot: u64,
    snapshot_seq: u64,
    snapshot_lsn: Lsn,
    dirty_since_lsn: Option<Lsn>,
    snapshot_at: Option<DateTime<Utc>>,
}

impl Bookkeeping {
    fn of(st: &ShardState) -> Self {
        Self {
            records_since_snapshot: st.records_since_snapshot,
            snapshot_seq: st.snapshot_seq,
            snapshot_lsn: st.snapshot_lsn,
            dirty_since_lsn: st.dirty_since_lsn,
            snapshot_at: st.snapshot_at,
        }
    }
}
