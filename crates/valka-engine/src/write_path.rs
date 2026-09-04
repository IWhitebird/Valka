//! The write path shared by every command: validate → apply → append → (durable) → publish.

use chrono::{DateTime, Utc};
use tracing::warn;
use valka_core::{ServerError, ShardId, TaskStatus, shard_of_task_id};
use valka_wal::{Durable, Envelope, FailureOutcome, WalRecord};

use crate::engine::{Engine, EngineEvent, PendingKey};
use crate::sink::DispatchableTask;
use crate::state::{RunStatus, ShardState, Transition};
use crate::timers::TimerKind;

use crate::retry::compute_retry_delay;
use valka_core::SchedulerConfig;

impl Engine {
    pub(crate) fn shard_for(&self, task_id: &str) -> Result<ShardId, ServerError> {
        let shard =
            shard_of_task_id(task_id).ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
        if !self.owns(shard) {
            return Err(ServerError::NotOwner(shard.0));
        }
        Ok(shard)
    }

    /// Validate + build a record under the shard lock, apply it, append it. Returns the
    /// durability future, the transitions, and whatever `after` reads from the new state.
    pub(crate) fn mutate<R, V>(
        &self,
        shard: ShardId,
        build: impl FnOnce(&ShardState) -> Result<(WalRecord, R), ServerError>,
        after: impl FnOnce(&ShardState) -> V,
    ) -> Result<(Durable, Vec<Transition>, R, V), ServerError> {
        let mut st = self.inner.shards[shard.0 as usize].lock();
        let (record, ret) = build(&st)?;
        let mut env = Envelope::with_ts(shard, self.inner.clock.now(), record);
        let was_clean = st.records_since_snapshot == 0;
        let transitions = st.apply(&mut env);
        if was_clean {
            st.dirty_since_lsn = Some(self.inner.writer.next_lsn());
        }
        let view = after(&st);
        let durable = self.inner.writer.append(vec![env]);
        drop(st);
        // Tasks that left PENDING must leave the runnable index right away.
        self.drop_from_pending(&transitions);
        Ok((durable, transitions, ret, view))
    }

    pub(crate) fn arm_timers_for(&self, transitions: &[Transition]) {
        for tr in transitions {
            if tr.to == Some(TaskStatus::Retry) {
                let Some(shard) = shard_of_task_id(&tr.task_id) else {
                    continue;
                };
                let at = self.inner.shards[shard.0 as usize]
                    .lock()
                    .tasks
                    .get(&tr.task_id)
                    .and_then(|t| t.next_attempt_at);
                if let Some(at) = at {
                    self.inner.timers.lock().schedule(
                        at,
                        TimerKind::Promote {
                            task_id: tr.task_id.clone(),
                        },
                    );
                }
            }
        }
    }

    pub(crate) fn spawn_after_durable(&self, durable: Durable, transitions: Vec<Transition>) {
        let me = self.clone();
        tokio::spawn(async move {
            match durable.wait().await {
                Ok(_) => me.after_durable(&transitions),
                Err(e) => warn!(error = %e, "record not durable; transitions not published"),
            }
        });
    }

    /// Runs once records are durable: publish events, index newly runnable tasks, offer
    /// them to the sink, arm timers.
    pub(crate) fn after_durable(&self, transitions: &[Transition]) {
        let now = self.inner.clock.now();
        for tr in transitions {
            let _ = self.inner.events.send(EngineEvent {
                task_id: tr.task_id.clone(),
                queue_name: tr.queue_name.clone(),
                previous: tr.from,
                new: tr.to,
                worker_id: tr.worker_id.clone(),
                node_id: self.inner.cfg.node_id.clone(),
                attempt: tr.attempt,
                error: tr.error.clone(),
                ts: now,
            });
            match tr.to {
                Some(TaskStatus::Completed) => {
                    valka_core::metrics::record_task_completed(&tr.queue_name)
                }
                Some(TaskStatus::Failed) => valka_core::metrics::record_task_failed(&tr.queue_name),
                Some(TaskStatus::Retry) => valka_core::metrics::record_task_retried(&tr.queue_name),
                Some(TaskStatus::DeadLetter) => {
                    valka_core::metrics::record_task_dead_lettered(&tr.queue_name)
                }
                _ => {}
            }
            if tr.to == Some(TaskStatus::Pending) {
                self.index_or_offer(&tr.task_id);
            }
        }
        self.arm_timers_for(transitions);
    }
}

pub(crate) fn pending_key(t: &crate::state::TaskState) -> PendingKey {
    (
        -t.spec.priority,
        t.spec.created_at.timestamp_millis(),
        t.spec.id.clone(),
    )
}

pub(crate) fn dispatchable(t: &crate::state::TaskState, attempt: i32) -> DispatchableTask {
    DispatchableTask {
        task_id: t.spec.id.clone(),
        queue_name: t.spec.queue_name.clone(),
        task_name: t.spec.task_name.clone(),
        input: t.spec.input.clone(),
        attempt_number: attempt,
        timeout_seconds: t.spec.timeout_seconds,
        metadata: t.spec.metadata.clone(),
        priority: t.spec.priority,
    }
}

pub(crate) fn ensure_running(t: &crate::state::TaskState, run_id: &str) -> Result<(), ServerError> {
    if t.status != TaskStatus::Running {
        return Err(ServerError::InvalidStatusTransition {
            from: t.status.as_str().into(),
            to: "FINISHED".into(),
        });
    }
    match t.runs.iter().find(|r| r.id == run_id) {
        Some(r) if r.status == RunStatus::Running => Ok(()),
        Some(_) => Err(ServerError::InvalidStatusTransition {
            from: "FINISHED_RUN".into(),
            to: "FINISHED".into(),
        }),
        None => Err(ServerError::LeaseExpired(format!("run {run_id} not found"))),
    }
}

pub(crate) fn decide_outcome(
    attempt: i32,
    max_retries: i32,
    retryable: bool,
    now: DateTime<Utc>,
    sched: &SchedulerConfig,
) -> FailureOutcome {
    if retryable && attempt < max_retries {
        FailureOutcome::Retry {
            at: now
                + compute_retry_delay(
                    attempt,
                    sched.retry_base_delay_secs,
                    sched.retry_max_delay_secs,
                ),
        }
    } else if attempt >= max_retries {
        FailureOutcome::DeadLetter
    } else {
        FailureOutcome::Failed
    }
}
