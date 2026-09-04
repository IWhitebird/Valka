//! Post-replay index rebuild and the snapshot / truncation round.

use chrono::Duration as ChronoDuration;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use valka_core::{ShardId, TaskStatus};
use valka_wal::{Lsn, reader, snapshot};

use crate::engine::Engine;
use crate::state::RunStatus;
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
        loop {
            tokio::select! {
                _ = shutdown.changed() => { if *shutdown.borrow() { return; } }
                _ = tokio::time::sleep(interval) => {}
            }
            self.snapshot_dirty_shards(false).await;
        }
    }

    /// Snapshot every shard with uncovered records (or, when `force`, every dirty shard
    /// regardless of the record threshold), then prune snapshots and truncate segments.
    pub(crate) async fn snapshot_dirty_shards(&self, force: bool) {
        let threshold = self.inner.cfg.wal.snapshot_after_records;
        let mut written = 0usize;
        for (i, m) in self.inner.shards.iter().enumerate() {
            let (snap, lsn, prev) = {
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
                let prev = (
                    st.records_since_snapshot,
                    st.snapshot_seq,
                    st.snapshot_lsn,
                    st.dirty_since_lsn,
                    st.snapshot_at,
                );
                let taken_at = self.inner.clock.now();
                let snap = st.to_snapshot(taken_at);
                st.records_since_snapshot = 0;
                st.snapshot_seq = st.shard_seq;
                st.snapshot_lsn = lsn;
                st.dirty_since_lsn = None;
                st.snapshot_at = Some(taken_at);
                (snap, lsn, prev)
            };
            // The snapshot claims to cover records in segments < lsn; those records must
            // be durable before the snapshot may be relied upon for truncation. Wait.
            if self.inner.writer.durable_lsn() < Lsn::new(lsn.epoch, lsn.seq.saturating_sub(1))
                && let Err(e) = self.inner.writer.sync().await
            {
                warn!(error = %e, "sync before snapshot failed");
            }
            let shard = ShardId(i as u16);
            match snapshot::write(&self.inner.store, shard, lsn, &snap).await {
                Ok(()) => {
                    written += 1;
                    if let Err(e) = snapshot::prune(
                        &self.inner.store,
                        shard,
                        self.inner.cfg.wal.snapshots_to_keep.max(1),
                    )
                    .await
                    {
                        warn!(%shard, error = %e, "snapshot prune failed");
                    }
                }
                Err(e) => {
                    error!(%shard, error = %e, "snapshot write failed; keeping WAL");
                    let mut st = m.lock();
                    st.records_since_snapshot += prev.0;
                    st.snapshot_seq = prev.1;
                    st.snapshot_lsn = prev.2;
                    st.dirty_since_lsn = prev.3.or(Some(lsn));
                    st.snapshot_at = prev.4;
                }
            }
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
}
