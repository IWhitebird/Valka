//! Background loops: the timer ticker (lease expiry, retry promotion, retention), the
//! coalesced heartbeat flush, and the feeder that tops up the matching layer.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::time::Duration;
use tracing::{debug, warn};
use valka_core::{ServerError, TaskStatus};
use valka_wal::{Envelope, WalRecord};

use crate::engine::{Engine, LeaseDirty};
use crate::sink::OfferOutcome;
use crate::state::RunStatus;
use crate::timers::TimerKind;
use crate::write_path::decide_outcome;
use valka_core::ShardId;

impl Engine {
    pub(crate) fn spawn_loops(&self) {
        let me = self.clone();
        tokio::spawn(async move { me.ticker().await });
        let me = self.clone();
        tokio::spawn(async move { me.feeder().await });
        let me = self.clone();
        tokio::spawn(async move { me.snapshotter().await });
        let me = self.clone();
        tokio::spawn(async move { me.control_loop().await });
        tokio::spawn(
            self.inner
                .lease
                .clone()
                .run(self.inner.lease_stop.subscribe()),
        );
    }

    async fn ticker(self) {
        let mut shutdown = self.inner.shutdown.subscribe();
        let mut tick = tokio::time::interval(Duration::from_millis(
            self.inner.cfg.scheduler.timer_tick_ms.max(1),
        ));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut sampled = tokio::time::Instant::now();
        loop {
            tokio::select! {
                r = shutdown.changed() => { if r.is_err() || *shutdown.borrow() { return; } }
                _ = tick.tick() => {
                    self.fire_due_timers();
                    self.flush_lease_records();
                    self.lapse_offers();
                    if sampled.elapsed() >= Duration::from_secs(1) {
                        sampled = tokio::time::Instant::now();
                        self.sample_tasks_in_ram();
                    }
                }
            }
        }
    }

    /// Count the tasks held in RAM. Near the limit, finished tasks leave RAM early (their
    /// history stays in the bucket); a minute is kept so that a retried create still finds
    /// its idempotency key.
    pub(crate) fn sample_tasks_in_ram(&self) {
        let cap = self.inner.cfg.limits.max_tasks_in_ram;
        let count = |me: &Self| -> usize {
            me.inner
                .shards
                .iter()
                .map(|m| {
                    let st = m.lock();
                    if st.serving() { st.tasks.len() } else { 0 }
                })
                .sum()
        };
        let mut n = count(self);
        if cap > 0 && n.saturating_mul(10) >= cap.saturating_mul(9) {
            let cutoff = self.inner.clock.now() - ChronoDuration::seconds(60);
            let mut evicted = 0;
            for m in &self.inner.shards {
                let mut st = m.lock();
                if st.serving() {
                    evicted += st.evict_terminal_before(cutoff);
                }
            }
            if evicted > 0 {
                warn!(
                    evicted,
                    tasks = n,
                    cap,
                    "near the task limit: evicted finished tasks early"
                );
                n = count(self);
            }
        }
        self.inner
            .tasks_in_ram
            .store(n, std::sync::atomic::Ordering::Relaxed);
        valka_core::metrics::set_tasks_in_ram(n as f64);
    }

    fn fire_due_timers(&self) {
        let now = self.inner.clock.now();
        let due = self.inner.timers.lock().due(now);
        for kind in due {
            match kind {
                TimerKind::Promote { task_id } => self.promote(&task_id, now),
                TimerKind::LeaseExpiry { task_id, run_id } => {
                    self.expire_lease(&task_id, &run_id, now)
                }
                TimerKind::Unhold { shard } => self.unhold_shard(ShardId(shard)),
                TimerKind::Evict => {
                    let retention =
                        ChronoDuration::seconds(self.inner.cfg.wal.completed_retention_secs as i64);
                    let mut n = 0;
                    for m in &self.inner.shards {
                        let mut st = m.lock();
                        if st.serving() {
                            n += st.evict_terminal_before(now - retention);
                        }
                    }
                    if n > 0 {
                        debug!(evicted = n, "retention sweep");
                    }
                    self.inner
                        .timers
                        .lock()
                        .schedule(now + ChronoDuration::seconds(60), TimerKind::Evict);
                }
            }
        }
    }

    fn promote(&self, task_id: &str, now: DateTime<Utc>) {
        let Some(shard) = self.shard_of(task_id) else {
            return;
        };
        let res = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                let waiting = matches!(t.status, TaskStatus::Retry)
                    || (t.status == TaskStatus::Pending && !t.due);
                if !waiting {
                    return Err(ServerError::Internal("not waiting".into()));
                }
                if let Some(at) = t.next_attempt_at
                    && at > now
                {
                    return Err(ServerError::Internal(format!(
                        "rearm:{}",
                        at.timestamp_millis()
                    )));
                }
                Ok((
                    WalRecord::TaskPromoted {
                        task_id: task_id.into(),
                    },
                    (),
                ))
            },
            |_| (),
        );
        match res {
            Ok((d, tr, _, _)) => self.spawn_after_durable(d, tr),
            Err(ServerError::Internal(m)) if m.starts_with("rearm:") => {
                let ms: i64 = m[6..].parse().unwrap_or(0);
                if let Some(at) = DateTime::from_timestamp_millis(ms) {
                    self.inner.timers.lock().schedule(
                        at,
                        TimerKind::Promote {
                            task_id: task_id.into(),
                        },
                    );
                }
            }
            Err(_) => {}
        }
    }

    fn expire_lease(&self, task_id: &str, run_id: &str, now: DateTime<Utc>) {
        let Some(shard) = self.shard_of(task_id) else {
            return;
        };
        let sched = self.inner.cfg.scheduler.clone();
        let res = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                if t.status != TaskStatus::Running {
                    return Err(ServerError::Internal("not running".into()));
                }
                let run = t
                    .runs
                    .iter()
                    .find(|r| r.id == run_id)
                    .ok_or_else(|| ServerError::Internal("no run".into()))?;
                if run.status != RunStatus::Running {
                    return Err(ServerError::Internal("run finished".into()));
                }
                if run.lease_until > now {
                    return Err(ServerError::Internal(format!(
                        "rearm:{}",
                        run.lease_until.timestamp_millis()
                    )));
                }
                let outcome =
                    decide_outcome(t.attempt_count, t.spec.max_retries, true, now, &sched);
                Ok((
                    WalRecord::LeaseExpired {
                        task_id: task_id.into(),
                        run_id: run_id.into(),
                        error: "Lease expired".into(),
                        outcome,
                    },
                    (),
                ))
            },
            |_| (),
        );
        match res {
            Ok((d, tr, _, _)) => {
                warn!(task_id, run_id, "lease expired");
                self.arm_timers_for(&tr);
                self.spawn_after_durable(d, tr);
            }
            Err(ServerError::Internal(m)) if m.starts_with("rearm:") => {
                let ms: i64 = m[6..].parse().unwrap_or(0);
                if let Some(at) = DateTime::from_timestamp_millis(ms) {
                    self.inner.timers.lock().schedule(
                        at,
                        TimerKind::LeaseExpiry {
                            task_id: task_id.into(),
                            run_id: run_id.into(),
                        },
                    );
                }
            }
            Err(_) => {}
        }
    }

    pub(crate) fn flush_lease_records(&self) {
        self.flush_lease_records_for(None);
    }

    /// Write coalesced heartbeat records: all of them, or only one shard's.
    pub(crate) fn flush_lease_records_for(&self, only: Option<ShardId>) {
        let dirty: LeaseDirty = {
            let mut d = self.inner.lease_dirty.lock();
            match only {
                None => std::mem::take(&mut *d),
                Some(s) => {
                    let (mine, rest): (LeaseDirty, LeaseDirty) = std::mem::take(&mut *d)
                        .into_iter()
                        .partition(|(_, v)| v.0 == s);
                    *d = rest;
                    mine
                }
            }
        };
        if dirty.is_empty() {
            return;
        }
        // Per shard: assign shard_seqs and append in one critical section, so the log holds
        // each shard's records in shard_seq order (replay and splits rely on it).
        let mut by_shard: std::collections::BTreeMap<
            ShardId,
            Vec<(String, String, DateTime<Utc>)>,
        > = std::collections::BTreeMap::new();
        for (task_id, (shard, run_id, lease_until)) in dirty {
            by_shard
                .entry(shard)
                .or_default()
                .push((task_id, run_id, lease_until));
        }
        for (shard, leases) in by_shard {
            let mut st = self.inner.shards[shard.index()].lock();
            // Serving, or releasing (whose final snapshot must include it).
            if !matches!(
                st.phase,
                crate::state::ShardPhase::Serving | crate::state::ShardPhase::Releasing
            ) {
                continue;
            }
            let was_clean = st.records_since_snapshot == 0;
            let mut envs = Vec::with_capacity(leases.len());
            for (task_id, run_id, lease_until) in leases {
                let mut env = Envelope::with_ts(
                    shard,
                    self.inner.clock.now(),
                    WalRecord::LeaseExtended {
                        task_id,
                        run_id,
                        lease_until,
                    },
                );
                env.generation = st.generation;
                st.apply(&mut env);
                envs.push(env);
            }
            if was_clean {
                st.dirty_since_lsn = Some(self.inner.writer.next_lsn());
            }
            drop(self.inner.writer.append(envs));
        }
    }

    async fn feeder(self) {
        let mut shutdown = self.inner.shutdown.subscribe();
        let interval = self.inner.cfg.feeder_interval;
        let mut rejected = false;
        loop {
            if rejected {
                // A task went back because the slot counted for it was gone. Handing it
                // back woke this loop again: wait the interval rather than spin on it.
                tokio::select! {
                    r = shutdown.changed() => { if r.is_err() || *shutdown.borrow() { return; } }
                    _ = tokio::time::sleep(interval) => {}
                }
            } else {
                tokio::select! {
                    r = shutdown.changed() => { if r.is_err() || *shutdown.borrow() { return; } }
                    _ = self.inner.pending_wake.notified() => {}
                    _ = tokio::time::sleep(interval) => {}
                }
            }
            // Wake-ups can arrive faster than rounds finish; let other tasks on this thread
            // (the WAL committer, a worker's match loop) run between rounds.
            tokio::task::yield_now().await;
            rejected = false;
            let queues: Vec<String> = self.inner.pending.lock().keys().cloned().collect();
            if queues.is_empty() {
                continue;
            }
            let sink = self.inner.sink.read().clone();
            for q in queues {
                let cap = sink.capacity(&q).min(self.inner.cfg.feeder_batch_size);
                if cap == 0 {
                    continue;
                }
                let mut refused = false;
                for task in self.take_pending(&q, cap) {
                    // After one rejection the rest of the batch goes straight back: each
                    // was taken out of the index and must not wait for its offer to lapse.
                    let id = task.task_id.clone();
                    if refused || sink.offer(task) == OfferOutcome::Rejected {
                        self.unoffer(&id);
                        refused = true;
                        continue;
                    }
                    valka_core::metrics::record_async_match();
                }
                rejected |= refused;
                valka_core::metrics::set_pending_tasks(&q, self.pending_count(&q) as f64);
            }
        }
    }
}
