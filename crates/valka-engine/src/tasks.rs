//! Task commands: create, read, cancel, delete, dispatch, complete, fail, heartbeat.

use chrono::Duration as ChronoDuration;
use serde_json::Value;
use valka_core::{ServerError, ShardId, TaskStatus, shard_of_task_id};
use valka_wal::{TaskSpec, WalRecord};

use crate::engine::{CreateTask, DispatchInfo, Engine, FailResult};
use crate::state::RunStatus;
use crate::timers::TimerKind;
use crate::view::{DeadLetterView, RunView, TaskView};
use crate::write_path::{decide_outcome, dispatchable, ensure_running};

impl Engine {
    pub async fn create_task(&self, req: CreateTask) -> Result<TaskView, ServerError> {
        if req.queue_name.is_empty() || req.task_name.is_empty() {
            return Err(ServerError::InvalidArgument(
                "queue_name and task_name are required".into(),
            ));
        }
        let (uuid, shard) =
            valka_core::new_task_uuid(&req.queue_name, req.idempotency_key.as_deref());
        let id = uuid.to_string();
        let now = self.inner.clock.now();
        let spec = TaskSpec {
            id: id.clone(),
            queue_name: req.queue_name.clone(),
            task_name: req.task_name,
            input: req.input,
            priority: req.priority,
            max_retries: req.max_retries,
            timeout_seconds: req.timeout_seconds,
            idempotency_key: req.idempotency_key.clone(),
            metadata: req.metadata,
            scheduled_at: req.scheduled_at,
            created_at: now,
        };
        let (durable, transitions, _, view) = self.mutate(
            shard,
            |st| {
                if let Some(k) = &req.idempotency_key
                    && st.idempotency.contains_key(k)
                {
                    return Err(ServerError::IdempotencyConflict(k.clone()));
                }
                Ok((WalRecord::TaskCreated { task: spec.clone() }, ()))
            },
            |st| st.tasks.get(&id).map(|t| t.view()),
        )?;
        durable.wait().await?;
        valka_core::metrics::record_task_created(&req.queue_name);
        self.after_durable(&transitions);
        view.ok_or_else(|| ServerError::Internal("task vanished after create".into()))
    }

    pub fn get_task(&self, task_id: &str) -> Option<TaskView> {
        let shard = shard_of_task_id(task_id)?;
        self.inner.shards[shard.0 as usize]
            .lock()
            .tasks
            .get(task_id)
            .map(|t| t.view())
    }

    pub fn list_tasks(
        &self,
        queue: Option<&str>,
        status: Option<TaskStatus>,
        limit: usize,
        offset: usize,
    ) -> Vec<TaskView> {
        let mut all: Vec<TaskView> = Vec::new();
        for m in &self.inner.shards {
            let st = m.lock();
            for t in st.tasks.values() {
                if queue.is_some_and(|q| q != t.spec.queue_name) {
                    continue;
                }
                if status.is_some_and(|s| s != t.status) {
                    continue;
                }
                all.push(t.view());
            }
        }
        all.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        all.into_iter().skip(offset).take(limit).collect()
    }

    pub fn count_tasks(&self, queue: Option<&str>, status: Option<TaskStatus>) -> usize {
        self.inner
            .shards
            .iter()
            .map(|m| {
                m.lock()
                    .tasks
                    .values()
                    .filter(|t| {
                        queue.is_none_or(|q| q == t.spec.queue_name)
                            && !status.is_some_and(|s| s != t.status)
                    })
                    .count()
            })
            .sum()
    }

    /// Cancel a non-terminal task. Returns the view and the worker to notify if running.
    pub async fn cancel_task(
        &self,
        task_id: &str,
        reason: &str,
    ) -> Result<(TaskView, Option<String>), ServerError> {
        let shard = self.shard_for(task_id)?;
        let (durable, transitions, _, view) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                if t.is_terminal() {
                    return Err(ServerError::InvalidStatusTransition {
                        from: t.status.as_str().into(),
                        to: "CANCELLED".into(),
                    });
                }
                Ok((
                    WalRecord::TaskCancelled {
                        task_id: task_id.into(),
                        reason: reason.into(),
                    },
                    (),
                ))
            },
            |st| st.tasks.get(task_id).map(|t| t.view()),
        )?;
        durable.wait().await?;
        self.after_durable(&transitions);
        let worker = transitions.first().and_then(|t| t.worker_id.clone());
        let view =
            view.ok_or_else(|| ServerError::Internal("task vanished after cancel".into()))?;
        Ok((view, worker))
    }

    pub async fn delete_task(&self, task_id: &str) -> Result<bool, ServerError> {
        let Some(shard) = shard_of_task_id(task_id) else {
            return Ok(false);
        };
        let res = self.mutate(
            shard,
            |st| {
                if !st.tasks.contains_key(task_id) {
                    return Err(ServerError::TaskNotFound(task_id.into()));
                }
                Ok((
                    WalRecord::TaskDeleted {
                        task_id: task_id.into(),
                    },
                    (),
                ))
            },
            |_| (),
        );
        match res {
            Ok((durable, transitions, _, _)) => {
                durable.wait().await?;
                self.after_durable(&transitions);
                Ok(true)
            }
            Err(ServerError::TaskNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Drop every task in every owned shard. Returns the count removed.
    pub async fn clear_all_tasks(&self) -> Result<usize, ServerError> {
        let mut total = 0usize;
        let mut waits = Vec::new();
        let mut all_transitions = Vec::new();
        for shard in ShardId::all() {
            let res = self.mutate(
                shard,
                |st| {
                    if st.is_empty() {
                        return Err(ServerError::Internal("empty".into()));
                    }
                    Ok((WalRecord::ShardCleared, st.tasks.len()))
                },
                |_| (),
            );
            if let Ok((d, tr, n, _)) = res {
                total += n;
                waits.push(d);
                all_transitions.extend(tr);
            }
        }
        for d in waits {
            d.wait().await?;
        }
        self.after_durable(&all_transitions);
        Ok(total)
    }

    /// Record a dispatch. Does **not** wait for durability: the dispatch record is
    /// asynchronous by design (see DESIGN.md §5); a lost dispatch record only costs a
    /// possible duplicate execution, which the at-least-once contract already allows.
    pub fn dispatch(&self, task_id: &str, worker_id: &str) -> Result<DispatchInfo, ServerError> {
        let shard = self.shard_for(task_id)?;
        let now = self.inner.clock.now();
        let run_id = valka_core::shard::embed_shard(uuid::Uuid::now_v7(), shard).to_string();
        let node_id = self.inner.cfg.node_id.clone();
        let grace = self.inner.cfg.scheduler.lease_grace_secs;
        let (durable, transitions, info, _) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                if t.status != TaskStatus::Pending || !t.due {
                    return Err(ServerError::InvalidStatusTransition {
                        from: t.status.as_str().into(),
                        to: "RUNNING".into(),
                    });
                }
                let attempt = t.attempt_count + 1;
                let lease_until =
                    now + ChronoDuration::seconds(t.spec.timeout_seconds as i64 + grace);
                let info = DispatchInfo {
                    run_id: run_id.clone(),
                    attempt,
                    lease_until,
                    task: dispatchable(t, attempt),
                    checkpoints: t.checkpoint_views(),
                };
                Ok((
                    WalRecord::TaskDispatched {
                        task_id: task_id.into(),
                        run_id: run_id.clone(),
                        attempt,
                        worker_id: worker_id.into(),
                        node_id: node_id.clone(),
                        lease_until,
                    },
                    info,
                ))
            },
            |_| (),
        )?;
        self.inner.timers.lock().schedule(
            info.lease_until,
            TimerKind::LeaseExpiry {
                task_id: task_id.into(),
                run_id: info.run_id.clone(),
            },
        );
        self.spawn_after_durable(durable, transitions);
        Ok(info)
    }

    pub async fn complete_run(
        &self,
        task_id: &str,
        run_id: &str,
        output: Option<Value>,
    ) -> Result<TaskView, ServerError> {
        let shard = self.shard_for(task_id)?;
        let (durable, transitions, _, view) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                ensure_running(t, run_id)?;
                Ok((
                    WalRecord::RunCompleted {
                        task_id: task_id.into(),
                        run_id: run_id.into(),
                        output: output.clone(),
                    },
                    (),
                ))
            },
            |st| st.tasks.get(task_id).map(|t| t.view()),
        )?;
        durable.wait().await?;
        self.after_durable(&transitions);
        view.ok_or_else(|| ServerError::Internal("task vanished after completion".into()))
    }

    pub async fn fail_run(
        &self,
        task_id: &str,
        run_id: &str,
        error: &str,
        retryable: bool,
    ) -> Result<FailResult, ServerError> {
        let shard = self.shard_for(task_id)?;
        let now = self.inner.clock.now();
        let sched = self.inner.cfg.scheduler.clone();
        let (durable, transitions, outcome, _) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                ensure_running(t, run_id)?;
                let outcome =
                    decide_outcome(t.attempt_count, t.spec.max_retries, retryable, now, &sched);
                Ok((
                    WalRecord::RunFailed {
                        task_id: task_id.into(),
                        run_id: run_id.into(),
                        error: error.into(),
                        outcome: outcome.clone(),
                    },
                    outcome,
                ))
            },
            |_| (),
        )?;
        self.arm_timers_for(&transitions);
        durable.wait().await?;
        self.after_durable(&transitions);
        Ok(FailResult { outcome })
    }

    /// Extend leases in RAM now; the WAL record is coalesced by the ticker.
    pub fn heartbeat(&self, task_ids: &[String]) {
        let now = self.inner.clock.now();
        let lease = now + ChronoDuration::seconds(self.inner.cfg.scheduler.heartbeat_lease_secs);
        for task_id in task_ids {
            let Some(shard) = shard_of_task_id(task_id) else {
                continue;
            };
            let mut st = self.inner.shards[shard.0 as usize].lock();
            let Some(t) = st.tasks.get_mut(task_id) else {
                continue;
            };
            if t.status != TaskStatus::Running {
                continue;
            }
            let Some(run) = t.current_run_mut() else {
                continue;
            };
            if run.status != RunStatus::Running {
                continue;
            }
            run.lease_until = run.lease_until.max(lease);
            run.last_heartbeat = now;
            let run_id = run.id.clone();
            drop(st);
            self.inner
                .lease_dirty
                .lock()
                .insert(task_id.clone(), (shard, run_id, lease));
        }
    }

    pub fn runs_for_task(&self, task_id: &str) -> Option<Vec<RunView>> {
        let shard = shard_of_task_id(task_id)?;
        self.inner.shards[shard.0 as usize]
            .lock()
            .tasks
            .get(task_id)
            .map(|t| t.run_views())
    }

    pub fn list_dead_letters(
        &self,
        queue: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Vec<DeadLetterView> {
        let mut all: Vec<DeadLetterView> = Vec::new();
        for m in &self.inner.shards {
            let st = m.lock();
            all.extend(
                st.dead_letters
                    .values()
                    .filter(|d| queue.is_none_or(|q| q == d.queue_name))
                    .map(|d| d.view()),
            );
        }
        all.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        all.into_iter().skip(offset).take(limit).collect()
    }
}
