//! Background loops: the timer ticker (lease expiry, retry promotion, retention), the
//! coalesced heartbeat flush, and the feeder that tops up the matching layer.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::time::Duration;
use tracing::{debug, warn};
use valka_core::{ServerError, TaskStatus, shard_of_task_id};
use valka_wal::{Envelope, WalRecord};

use crate::engine::{Engine, LeaseDirty};
use crate::sink::OfferOutcome;
use crate::state::RunStatus;
use crate::timers::TimerKind;
use crate::write_path::decide_outcome;

impl Engine {
    pub(crate) fn spawn_loops(&self) {
        let me = self.clone();
        tokio::spawn(async move { me.ticker().await });
        let me = self.clone();
        tokio::spawn(async move { me.feeder().await });
        let me = self.clone();
        tokio::spawn(async move { me.snapshotter().await });
    }

    async fn ticker(self) {
        let mut shutdown = self.inner.shutdown.subscribe();
        let mut tick = tokio::time::interval(Duration::from_millis(
            self.inner.cfg.scheduler.timer_tick_ms.max(1),
        ));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.changed() => { if *shutdown.borrow() { return; } }
                _ = tick.tick() => {
                    self.fire_due_timers();
                    self.flush_lease_records();
                }
            }
        }
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
                TimerKind::Evict => {
                    let retention =
                        ChronoDuration::seconds(self.inner.cfg.wal.completed_retention_secs as i64);
                    let mut n = 0;
                    for m in &self.inner.shards {
                        n += m.lock().evict_terminal_before(now - retention);
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
        let Some(shard) = shard_of_task_id(task_id) else {
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
        let Some(shard) = shard_of_task_id(task_id) else {
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
        let dirty: LeaseDirty = std::mem::take(&mut *self.inner.lease_dirty.lock());
        if dirty.is_empty() {
            return;
        }
        let mut envs = Vec::with_capacity(dirty.len());
        for (task_id, (shard, run_id, lease_until)) in dirty {
            let mut st = self.inner.shards[shard.0 as usize].lock();
            let mut env = Envelope::with_ts(
                shard,
                self.inner.clock.now(),
                WalRecord::LeaseExtended {
                    task_id,
                    run_id,
                    lease_until,
                },
            );
            let was_clean = st.records_since_snapshot == 0;
            st.apply(&mut env);
            if was_clean {
                st.dirty_since_lsn = Some(self.inner.writer.next_lsn());
            }
            envs.push(env);
        }
        drop(self.inner.writer.append(envs));
    }

    async fn feeder(self) {
        let mut shutdown = self.inner.shutdown.subscribe();
        let interval = self.inner.cfg.feeder_interval;
        loop {
            tokio::select! {
                _ = shutdown.changed() => { if *shutdown.borrow() { return; } }
                _ = self.inner.pending_wake.notified() => {}
                _ = tokio::time::sleep(interval) => {}
            }
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
                for task in self.take_pending(&q, cap) {
                    let id = task.task_id.clone();
                    if sink.offer(task) == OfferOutcome::Rejected {
                        self.unoffer(&id);
                        break;
                    }
                    valka_core::metrics::record_async_match();
                }
                valka_core::metrics::set_pending_tasks(&q, self.pending_count(&q) as f64);
            }
        }
    }
}
