//! Per-shard index rebuild after a claim, and the snapshot / truncation round.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use valka_core::{ShardId, TaskStatus};
use valka_wal::snapshot::{self, LogPtr, SnapshotKey};
use valka_wal::{Lsn, reader};

use crate::engine::Engine;
use crate::state::{RunStatus, ShardPhase, ShardSnapshot, ShardState};
use crate::timers::TimerKind;
use crate::write_path::pending_key;

impl Engine {
    /// A shard was just claimed: index its runnable tasks, arm its timers, give its
    /// RUNNING tasks a grace lease (their workers must re-handshake), apply retention.
    pub(crate) fn rebuild_shard_indexes(&self, shard: ShardId) {
        let now = self.inner.clock.now();
        let grace = ChronoDuration::seconds(self.inner.cfg.scheduler.recovery_grace_secs);
        let retention = ChronoDuration::seconds(self.inner.cfg.wal.completed_retention_secs as i64);
        let held = self
            .inner
            .hold_until
            .lock()
            .get(&shard)
            .is_some_and(|t| *t > tokio::time::Instant::now());
        let mut runnable = Vec::new();
        {
            let mut timers = self.inner.timers.lock();
            let mut st = self.inner.shards[shard.index()].lock();
            if st.phase != ShardPhase::Serving {
                return;
            }
            st.evict_terminal_before(now - retention);
            for t in st.tasks.values_mut() {
                t.offered = false;
                match t.status {
                    TaskStatus::Pending if t.due => {
                        runnable.push((t.spec.queue_name.clone(), pending_key(t)));
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
                        }
                    }
                    _ => {}
                }
            }
            if held {
                let at = now
                    + ChronoDuration::milliseconds(
                        self.inner.cfg.cluster.reconcile_grace_ms as i64,
                    );
                timers.schedule(at, TimerKind::Unhold { shard: shard.0 });
            }
        }
        if !held {
            let mut pending = self.inner.pending.lock();
            for (q, k) in runnable {
                pending.entry(q).or_default().insert(k);
            }
        }
        self.inner.pending_wake.notify_one();
    }

    /// The reconcile grace of a claimed shard ended: its recovered PENDING tasks may run.
    pub(crate) fn unhold_shard(&self, shard: ShardId) {
        self.inner.hold_until.lock().remove(&shard);
        let runnable: Vec<(String, crate::engine::PendingKey)> = {
            let st = self.inner.shards[shard.index()].lock();
            if !st.serving() {
                return;
            }
            st.tasks
                .values()
                .filter(|t| t.is_runnable())
                .map(|t| (t.spec.queue_name.clone(), pending_key(t)))
                .collect()
        };
        let mut pending = self.inner.pending.lock();
        for (q, k) in runnable {
            pending.entry(q).or_default().insert(k);
        }
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
            if self.poisoned().is_some() {
                return;
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

    /// Snapshot every serving shard with uncovered records (or, when `force`, every dirty
    /// shard regardless of the record threshold), then truncate this incarnation's log
    /// below the oldest record no snapshot covers.
    pub(crate) async fn snapshot_dirty_shards(&self, force: bool) {
        if force {
            self.inner
                .log_mark
                .store(self.inner.writer.bytes_committed(), Ordering::SeqCst);
        }
        let threshold = self.inner.cfg.wal.snapshot_after_records;
        let me = self.node_id().to_string();
        let inc = self.incarnation();
        let mut written = 0usize;
        let mut batch = Vec::with_capacity(SNAPSHOT_BATCH);
        for (i, m) in self.inner.shards.iter().enumerate() {
            {
                let mut st = m.lock();
                if st.phase != ShardPhase::Serving || st.records_since_snapshot == 0 {
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
                let key = SnapshotKey {
                    shard: ShardId(i as u16),
                    generation: st.generation,
                    seq: st.shard_seq,
                    ptr: LogPtr::At {
                        node: me.clone(),
                        inc,
                        seq: self.inner.writer.next_lsn().seq,
                    },
                };
                let prev = Bookkeeping::of(&st);
                let taken_at = self.inner.clock.now();
                let snap = st.to_snapshot(taken_at);
                st.records_since_snapshot = 0;
                st.snapshot_seq = st.shard_seq;
                st.snapshot_key = Some(key.clone());
                st.dirty_since_lsn = None;
                st.snapshot_at = Some(taken_at);
                batch.push(Captured { key, snap, prev });
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
        self.truncate_covered_log().await;
        debug!(snapshots = written, "snapshot round complete");
    }

    /// Delete this incarnation's segments below the oldest record no snapshot covers.
    /// The lease's truncation watermark is raised first, so a splitter never mistakes the
    /// hole for the end of the log.
    async fn truncate_covered_log(&self) {
        let durable = self.inner.writer.durable_lsn();
        let mut bound = Lsn::new(durable.inc, durable.seq + 1);
        for m in &self.inner.shards {
            let st = m.lock();
            if st.phase != ShardPhase::Unowned
                && let Some(d) = st.dirty_since_lsn
            {
                bound = bound.min(d);
            }
        }
        if bound.inc != self.incarnation() || bound.seq <= self.inner.lease.record().trunc {
            return;
        }
        if let Err(e) = self.inner.lease.raise_trunc(bound.seq).await {
            warn!(error = %e, "raising the truncation watermark failed; keeping the log");
            return;
        }
        match reader::truncate_before(
            &self.inner.store,
            self.node_id(),
            self.incarnation(),
            bound.seq,
        )
        .await
        {
            Ok(n) if n > 0 => info!(truncated = n, %bound, "log truncated"),
            Ok(_) => {}
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
            match snapshot::write(&self.inner.store, &c.key, &c.snap).await {
                Ok(_) => {
                    written += 1;
                    if let Err(e) = snapshot::prune(
                        &self.inner.store,
                        c.key.shard,
                        self.inner.cfg.wal.snapshots_to_keep.max(1),
                    )
                    .await
                    {
                        warn!(shard = %c.key.shard, error = %e, "snapshot prune failed");
                    }
                }
                Err(e) => {
                    error!(shard = %c.key.shard, error = %e, "snapshot write failed; keeping WAL");
                    self.restore_bookkeeping(&c);
                }
            }
        }
        written
    }

    fn restore_bookkeeping(&self, c: &Captured) {
        let mut st = self.inner.shards[c.key.shard.index()].lock();
        if st.phase == ShardPhase::Unowned {
            return;
        }
        st.records_since_snapshot += c.prev.records_since_snapshot;
        st.snapshot_seq = c.prev.snapshot_seq;
        st.snapshot_key = c.prev.snapshot_key.clone();
        let captured_at = match &c.key.ptr {
            LogPtr::At { inc, seq, .. } => Lsn::new(*inc, *seq),
            _ => Lsn::ZERO,
        };
        st.dirty_since_lsn = c.prev.dirty_since_lsn.or(Some(captured_at));
        st.snapshot_at = c.prev.snapshot_at;
    }
}

const SNAPSHOT_BATCH: usize = 64;

struct Captured {
    key: SnapshotKey,
    snap: ShardSnapshot,
    prev: Bookkeeping,
}

struct Bookkeeping {
    records_since_snapshot: u64,
    snapshot_seq: u64,
    snapshot_key: Option<SnapshotKey>,
    dirty_since_lsn: Option<Lsn>,
    snapshot_at: Option<DateTime<Utc>>,
}

impl Bookkeeping {
    fn of(st: &ShardState) -> Self {
        Self {
            records_since_snapshot: st.records_since_snapshot,
            snapshot_seq: st.snapshot_seq,
            snapshot_key: st.snapshot_key.clone(),
            dirty_since_lsn: st.dirty_since_lsn,
            snapshot_at: st.snapshot_at,
        }
    }
}
