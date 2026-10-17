//! Task commands: create, read, cancel, delete, dispatch, complete, fail, heartbeat.

use chrono::Duration as ChronoDuration;
use serde_json::Value;
use valka_core::{ServerError, ShardId, TaskStatus};
use valka_wal::{TaskSpec, WalRecord};

use crate::engine::{
    CreateTask, DispatchInfo, Engine, FailResult, ReattachOutcome, ResultOutcome, RunLocation,
    RunResult,
};
use crate::state::{RunEnd, RunStatus, SignalStatus};
use crate::timers::TimerKind;
use crate::view::{DeadLetterView, RunView, TaskView};
use crate::write_path::{decide_outcome, dispatchable, ensure_running};

/// How long a node trusts its cached copy of a queue's partition count.
const QUEUE_CONFIG_TTL: std::time::Duration = std::time::Duration::from_secs(30);

impl Engine {
    /// Create a task on a partition of its queue this node serves. Fails with `NotOwner`
    /// when placement picks a shard served elsewhere; the API layer routes instead (see
    /// [`Engine::place_task`] and [`Engine::create_task_at`]).
    pub async fn create_task(&self, req: CreateTask) -> Result<TaskView, ServerError> {
        let (id, _) = self.place_task(&req).await?;
        self.create_task_at(&id, req).await
    }

    /// The partition count of a queue (created with the default on first use).
    pub async fn queue_config(
        &self,
        queue: &str,
    ) -> Result<valka_wal::queues::QueueConfig, ServerError> {
        let cached = self.inner.queues.lock().get(queue).cloned();
        if let Some((c, at)) = &cached
            && at.elapsed() < QUEUE_CONFIG_TTL
        {
            return Ok(c.clone());
        }
        let mut delay = std::time::Duration::from_millis(20);
        for attempt in 0.. {
            match valka_wal::queues::get_or_create(
                &self.inner.store,
                queue,
                valka_core::DEFAULT_QUEUE_PARTITIONS,
            )
            .await
            {
                Ok(c) => {
                    self.inner
                        .queues
                        .lock()
                        .insert(queue.to_string(), (c.clone(), tokio::time::Instant::now()));
                    return Ok(c);
                }
                // A stale partition count only moves new tasks between partitions.
                Err(_) if cached.is_some() => return Ok(cached.unwrap().0),
                Err(e) if attempt >= 5 => {
                    return Err(ServerError::Unavailable(format!(
                        "reading queue {queue}: {e}"
                    )));
                }
                Err(_) => {
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
            }
        }
        unreachable!()
    }

    /// Drop the cached partition count of a queue (after an admin change).
    pub fn invalidate_queue(&self, queue: &str) {
        self.inner.queues.lock().remove(queue);
    }

    /// Choose the partition (and so the shard) for a new task and mint its id. With an
    /// idempotency key the partition is `jump_hash(key, P)`, so the key always lands on
    /// one shard and dedupes there. Without one, a partition this node serves is
    /// preferred (no hop), otherwise a random one.
    pub async fn place_task(&self, req: &CreateTask) -> Result<(String, ShardId), ServerError> {
        if req.queue_name.is_empty() || req.task_name.is_empty() {
            return Err(ServerError::InvalidArgument(
                "queue_name and task_name are required".into(),
            ));
        }
        if !self.is_serving() {
            return Err(ServerError::Unavailable(
                "this node's lease is not current; retry".into(),
            ));
        }
        let cfg = self.queue_config(&req.queue_name).await?;
        let n = self.inner.num_shards;
        let p = match &req.idempotency_key {
            Some(k) => valka_core::jump_hash(k, cfg.partitions),
            None => {
                let local: Vec<u32> = (0..cfg.partitions)
                    .filter(|p| self.owns(valka_core::partition_shard(&req.queue_name, *p, n)))
                    .collect();
                if local.is_empty() {
                    rand::random_range(0..cfg.partitions)
                } else {
                    local[rand::random_range(0..local.len())]
                }
            }
        };
        let shard = valka_core::partition_shard(&req.queue_name, p, n);
        Ok((valka_core::new_id_in_shard(shard).to_string(), shard))
    }

    /// Create a task under an id minted by [`Engine::place_task`] (possibly on another
    /// node). Creating the same id again returns the existing task, so a forwarded create
    /// can be retried safely.
    pub async fn create_task_at(&self, id: &str, req: CreateTask) -> Result<TaskView, ServerError> {
        if req.queue_name.is_empty() || req.task_name.is_empty() {
            return Err(ServerError::InvalidArgument(
                "queue_name and task_name are required".into(),
            ));
        }
        let shard = self
            .shard_of(id)
            .ok_or_else(|| ServerError::InvalidArgument(format!("bad task id {id}")))?;
        if let Some(existing) = self.get_task(id) {
            return Ok(existing);
        }
        let now = self.inner.clock.now();
        let id = id.to_string();
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

    /// The task holding idempotency key `key` in `shard`, if any.
    pub fn find_by_idempotency_key(&self, shard: ShardId, key: &str) -> Option<TaskView> {
        let st = self.inner.shards.get(shard.index())?.lock();
        let id = st.idempotency.get(key)?;
        st.tasks.get(id).map(|t| t.view())
    }

    pub fn get_task(&self, task_id: &str) -> Option<TaskView> {
        let shard = self.shard_of(task_id)?;
        self.inner.shards[shard.index()]
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
            if !st.serving() {
                continue;
            }
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
                let st = m.lock();
                if !st.serving() {
                    return 0;
                }
                st.tasks
                    .values()
                    .filter(|t| {
                        queue.is_none_or(|q| q == t.spec.queue_name)
                            && !status.is_some_and(|s| s != t.status)
                    })
                    .count()
            })
            .sum()
    }

    /// Cancel a non-terminal task. Returns the view and, if a run was in progress, where
    /// it lives (so the caller can tell its worker).
    pub async fn cancel_task(
        &self,
        task_id: &str,
        reason: &str,
    ) -> Result<(TaskView, Option<RunLocation>), ServerError> {
        let shard = self.shard_for(task_id)?;
        let (durable, transitions, running, view) = self.mutate(
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
                let running = t
                    .current_run()
                    .filter(|r| r.status == RunStatus::Running)
                    .map(|r| RunLocation {
                        run_id: r.id.clone(),
                        worker_id: r.worker_id.clone(),
                        node_id: r.node_id.clone(),
                    });
                Ok((
                    WalRecord::TaskCancelled {
                        task_id: task_id.into(),
                        reason: reason.into(),
                    },
                    running,
                ))
            },
            |st| st.tasks.get(task_id).map(|t| t.view()),
        )?;
        durable.wait().await?;
        self.after_durable(&transitions);
        let view =
            view.ok_or_else(|| ServerError::Internal("task vanished after cancel".into()))?;
        Ok((view, running))
    }

    /// Where a task's current run lives, if it is running.
    pub fn run_location(&self, task_id: &str) -> Option<RunLocation> {
        let shard = self.shard_of(task_id)?;
        let st = self.inner.shards[shard.index()].lock();
        let t = st.tasks.get(task_id)?;
        if t.status != TaskStatus::Running {
            return None;
        }
        t.current_run()
            .filter(|r| r.status == RunStatus::Running)
            .map(|r| RunLocation {
                run_id: r.id.clone(),
                worker_id: r.worker_id.clone(),
                node_id: r.node_id.clone(),
            })
    }

    pub async fn delete_task(&self, task_id: &str) -> Result<bool, ServerError> {
        let Some(shard) = self.shard_of(task_id) else {
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
        for shard in ShardId::all(self.inner.num_shards) {
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

    /// Record a dispatch to a worker of this node. See [`Engine::dispatch_to`].
    pub fn dispatch(&self, task_id: &str, worker_id: &str) -> Result<DispatchInfo, ServerError> {
        let node = self.inner.cfg.node_id.clone();
        self.dispatch_to(task_id, worker_id, &node, None)
    }

    /// Record a dispatch of a PENDING task to `worker_id`, attached to `node_id` (this node
    /// or the node that fetched the task). Does **not** wait for durability: the dispatch
    /// record is asynchronous by design (DESIGN.md §5); a lost dispatch record only costs
    /// a possible duplicate execution, which the at-least-once contract already allows.
    ///
    /// With `run_id` (minted by the caller for this task's shard) a retried call is
    /// idempotent: if that run is already the task's running run, its info is returned.
    pub fn dispatch_to(
        &self,
        task_id: &str,
        worker_id: &str,
        node_id: &str,
        run_id: Option<&str>,
    ) -> Result<DispatchInfo, ServerError> {
        let shard = self.shard_for(task_id)?;
        if let Some(rid) = run_id
            && let Some(info) = self.running_dispatch(shard, task_id, rid, worker_id)
        {
            return Ok(info);
        }
        let now = self.inner.clock.now();
        let run_id = match run_id {
            Some(r) if self.shard_of(r) == Some(shard) => r.to_string(),
            Some(r) => {
                return Err(ServerError::InvalidArgument(format!(
                    "run id {r} is not in task {task_id}'s shard"
                )));
            }
            None => valka_core::new_id_in_shard(shard).to_string(),
        };
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
                    pending_signals: st
                        .signals
                        .values()
                        .filter(|s| s.task_id == task_id && s.status == SignalStatus::Pending)
                        .map(|s| s.view())
                        .collect(),
                };
                Ok((
                    WalRecord::TaskDispatched {
                        task_id: task_id.into(),
                        run_id: run_id.clone(),
                        attempt,
                        worker_id: worker_id.into(),
                        node_id: node_id.into(),
                        lease_until,
                    },
                    info,
                ))
            },
            |_| (),
        )?;
        self.inner.remote_offers.lock().remove(task_id);
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

    /// The info of a dispatch that already happened (a retried `dispatch_to`).
    fn running_dispatch(
        &self,
        shard: ShardId,
        task_id: &str,
        run_id: &str,
        worker_id: &str,
    ) -> Option<DispatchInfo> {
        let st = self.inner.shards[shard.index()].lock();
        if !st.serving() {
            return None;
        }
        let t = st.tasks.get(task_id)?;
        let r = t.current_run()?;
        if t.status != TaskStatus::Running
            || r.id != run_id
            || r.worker_id != worker_id
            || r.status != RunStatus::Running
        {
            return None;
        }
        Some(DispatchInfo {
            run_id: r.id.clone(),
            attempt: r.attempt_number,
            lease_until: r.lease_until,
            task: dispatchable(t, r.attempt_number),
            checkpoints: t.checkpoint_views(),
            pending_signals: Vec::new(),
        })
    }

    /// A worker reconnected (through `node_id`) with a run in progress (PHASE2 §11.4):
    /// a current run is reattached to its new node; a run whose dispatch record was lost
    /// in a crash (task PENDING at exactly that attempt) is adopted under the worker's
    /// run id; anything else is stale.
    pub async fn reattach(
        &self,
        task_id: &str,
        run_id: &str,
        attempt: i32,
        worker_id: &str,
        node_id: &str,
    ) -> Result<ReattachOutcome, ServerError> {
        let shard = self.shard_for(task_id)?;
        let lease = self.inner.clock.now()
            + ChronoDuration::seconds(self.inner.cfg.scheduler.heartbeat_lease_secs);
        enum Plan {
            Reattach,
            Adopt,
            Stale,
        }
        let plan = {
            let st = self.inner.shards[shard.index()].lock();
            match st.tasks.get(task_id) {
                None => Plan::Stale,
                Some(t) => match t.status {
                    TaskStatus::Running
                        if t.current_run()
                            .is_some_and(|r| r.id == run_id && r.status == RunStatus::Running) =>
                    {
                        Plan::Reattach
                    }
                    TaskStatus::Pending
                        if t.due
                            && attempt == t.attempt_count + 1
                            && !t.runs.iter().any(|r| r.id == run_id) =>
                    {
                        Plan::Adopt
                    }
                    _ => Plan::Stale,
                },
            }
        };
        match plan {
            Plan::Stale => Ok(ReattachOutcome::Stale),
            Plan::Adopt => match self.dispatch_to(task_id, worker_id, node_id, Some(run_id)) {
                Ok(_) => Ok(ReattachOutcome::Adopted),
                Err(ServerError::InvalidStatusTransition { .. }) => Ok(ReattachOutcome::Stale),
                Err(e) => Err(e),
            },
            Plan::Reattach => {
                let res = self.mutate(
                    shard,
                    |st| {
                        let t = st
                            .tasks
                            .get(task_id)
                            .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                        ensure_running(t, run_id)?;
                        Ok((
                            WalRecord::RunReattached {
                                task_id: task_id.into(),
                                run_id: run_id.into(),
                                worker_id: worker_id.into(),
                                node_id: node_id.into(),
                                lease_until: lease,
                            },
                            (),
                        ))
                    },
                    |_| (),
                );
                match res {
                    Ok((durable, transitions, _, _)) => {
                        // Asynchronous like a dispatch record: losing it only costs the
                        // routing of a cancel or signal until the next reattach.
                        self.spawn_after_durable(durable, transitions);
                        Ok(ReattachOutcome::Reattached)
                    }
                    Err(ServerError::InvalidStatusTransition { .. })
                    | Err(ServerError::LeaseExpired(_)) => Ok(ReattachOutcome::Stale),
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// The run was dispatched but its worker never got the assignment: put the task back
    /// to PENDING without spending an attempt. Returns once durable.
    pub async fn return_run(&self, task_id: &str, run_id: &str) -> Result<(), ServerError> {
        let shard = self.shard_for(task_id)?;
        let (durable, transitions, _, _) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                ensure_running(t, run_id)?;
                Ok((
                    WalRecord::RunReturned {
                        task_id: task_id.into(),
                        run_id: run_id.into(),
                    },
                    (),
                ))
            },
            |_| (),
        )?;
        durable.wait().await?;
        self.after_durable(&transitions);
        Ok(())
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

    /// Record a worker's result idempotently. A result that cannot be recorded because the
    /// run already ended is `Applied` if it ended exactly this way, `Stale` otherwise. Both
    /// answers are given only once the state they describe is durable.
    pub async fn report_result(
        &self,
        task_id: &str,
        run_id: &str,
        result: RunResult,
    ) -> ResultOutcome {
        let recorded = match &result {
            RunResult::Completed(output) => self
                .complete_run(task_id, run_id, output.clone())
                .await
                .map(|_| ()),
            RunResult::Failed { error, retryable } => self
                .fail_run(task_id, run_id, error, *retryable)
                .await
                .map(|_| ()),
        };
        match recorded {
            Ok(()) => ResultOutcome::Applied,
            Err(e) if e.is_transient() => ResultOutcome::Retry,
            Err(_) => {
                let same = matches!(
                    (self.run_end(task_id, run_id), &result),
                    (Some(RunEnd::Completed), RunResult::Completed(_))
                        | (Some(RunEnd::Failed), RunResult::Failed { .. })
                );
                match self.sync().await {
                    Err(_) => ResultOutcome::Retry,
                    Ok(_) if same => ResultOutcome::Applied,
                    Ok(_) => ResultOutcome::Stale,
                }
            }
        }
    }

    fn run_end(&self, task_id: &str, run_id: &str) -> Option<RunEnd> {
        let shard = self.shard_of(task_id)?;
        self.inner.shards[shard.index()]
            .lock()
            .tasks
            .get(task_id)?
            .runs
            .iter()
            .find(|r| r.id == run_id)?
            .ended_by
    }

    /// Extend leases in RAM now; the WAL record is coalesced by the ticker.
    pub fn heartbeat(&self, task_ids: &[String]) {
        let now = self.inner.clock.now();
        let lease = now + ChronoDuration::seconds(self.inner.cfg.scheduler.heartbeat_lease_secs);
        for task_id in task_ids {
            let Some(shard) = self.shard_of(task_id) else {
                continue;
            };
            let mut st = self.inner.shards[shard.index()].lock();
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
        let shard = self.shard_of(task_id)?;
        self.inner.shards[shard.index()]
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
            if !st.serving() {
                continue;
            }
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
