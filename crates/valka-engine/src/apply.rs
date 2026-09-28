//! `ShardState::apply`: the single place state changes, at runtime and during replay.
//!
//! It is total: a record whose precondition no longer holds is a no-op, never a panic.
//! It returns the observable transitions so the engine can drive timers, events and the
//! pending index without re-deriving them.

use valka_core::TaskStatus;
use valka_wal::{Envelope, FailureOutcome, WalRecord};

use crate::state::{
    CheckpointState, DeadLetterEntry, RunEnd, RunState, RunStatus, ShardState, SignalState,
    SignalStatus, TaskState, Transition,
};

impl ShardState {
    pub fn apply(&mut self, env: &mut Envelope) -> Vec<Transition> {
        if env.shard_seq == 0 {
            self.shard_seq += 1;
            env.shard_seq = self.shard_seq;
        } else {
            if env.shard_seq <= self.shard_seq {
                return Vec::new();
            }
            self.shard_seq = env.shard_seq;
        }
        self.records_since_snapshot += 1;
        let now = env.ts();
        let mut out = Vec::new();

        match &env.record {
            WalRecord::TaskCreated { task } => {
                if self.tasks.contains_key(&task.id) {
                    return out;
                }
                if let Some(k) = &task.idempotency_key {
                    if self.idempotency.contains_key(k) {
                        return out;
                    }
                    self.idempotency.insert(k.clone(), task.id.clone());
                }
                let due = task.scheduled_at.is_none_or(|t| t <= now);
                let st = TaskState {
                    spec: task.clone(),
                    status: TaskStatus::Pending,
                    attempt_count: 0,
                    output: None,
                    error_message: None,
                    due,
                    next_attempt_at: if due { None } else { task.scheduled_at },
                    updated_at: now,
                    runs: Vec::new(),
                    signals: Vec::new(),
                    checkpoints: Vec::new(),
                    offered: false,
                };
                out.push(Transition {
                    task_id: task.id.clone(),
                    queue_name: task.queue_name.clone(),
                    from: None,
                    to: Some(TaskStatus::Pending),
                    attempt: 0,
                    worker_id: None,
                    error: None,
                });
                self.tasks.insert(task.id.clone(), st);
            }

            WalRecord::TaskDispatched {
                task_id,
                run_id,
                attempt,
                worker_id,
                node_id,
                lease_until,
            } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if t.status != TaskStatus::Pending || t.runs.iter().any(|r| r.id == *run_id) {
                    return out;
                }
                let from = t.status;
                t.status = TaskStatus::Running;
                t.attempt_count = *attempt;
                t.due = true;
                t.next_attempt_at = None;
                t.offered = false;
                t.updated_at = now;
                t.runs.push(RunState {
                    id: run_id.clone(),
                    attempt_number: *attempt,
                    worker_id: worker_id.clone(),
                    node_id: node_id.clone(),
                    status: RunStatus::Running,
                    output: None,
                    error_message: None,
                    lease_until: *lease_until,
                    started_at: now,
                    completed_at: None,
                    last_heartbeat: now,
                    ended_by: None,
                });
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(from),
                    to: Some(TaskStatus::Running),
                    attempt: *attempt,
                    worker_id: Some(worker_id.clone()),
                    error: None,
                });
            }

            WalRecord::RunCompleted {
                task_id,
                run_id,
                output,
            } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if t.status != TaskStatus::Running {
                    return out;
                }
                let Some(run) = t.run_mut(run_id) else {
                    return out;
                };
                if run.status != RunStatus::Running {
                    return out;
                }
                run.status = RunStatus::Completed;
                run.output = output.clone();
                run.completed_at = Some(now);
                run.ended_by = Some(RunEnd::Completed);
                let attempt = run.attempt_number;
                let worker = run.worker_id.clone();
                t.status = TaskStatus::Completed;
                t.output = output.clone();
                t.updated_at = now;
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(TaskStatus::Running),
                    to: Some(TaskStatus::Completed),
                    attempt,
                    worker_id: Some(worker),
                    error: None,
                });
            }

            WalRecord::RunFailed {
                task_id,
                run_id,
                error,
                outcome,
            }
            | WalRecord::LeaseExpired {
                task_id,
                run_id,
                outcome,
                error,
            } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if t.status != TaskStatus::Running {
                    return out;
                }
                let Some(run) = t.run_mut(run_id) else {
                    return out;
                };
                if run.status != RunStatus::Running {
                    return out;
                }
                run.status = RunStatus::Failed;
                run.error_message = Some(error.clone());
                run.completed_at = Some(now);
                run.ended_by = Some(match &env.record {
                    WalRecord::LeaseExpired { .. } => RunEnd::Expired,
                    _ => RunEnd::Failed,
                });
                let attempt = run.attempt_number;
                let worker = run.worker_id.clone();
                t.updated_at = now;
                let to = match outcome {
                    FailureOutcome::Retry { at } => {
                        t.status = TaskStatus::Retry;
                        t.due = false;
                        t.next_attempt_at = Some(*at);
                        t.error_message = Some(error.clone());
                        TaskStatus::Retry
                    }
                    FailureOutcome::Failed => {
                        t.status = TaskStatus::Failed;
                        t.error_message = Some(error.clone());
                        TaskStatus::Failed
                    }
                    FailureOutcome::DeadLetter => {
                        t.status = TaskStatus::DeadLetter;
                        t.error_message = Some(error.clone());
                        let entry = DeadLetterEntry {
                            id: uuid::Uuid::now_v7().to_string(),
                            task_id: task_id.clone(),
                            queue_name: t.spec.queue_name.clone(),
                            task_name: t.spec.task_name.clone(),
                            input: t.spec.input.clone(),
                            error_message: Some(error.clone()),
                            attempt_count: t.attempt_count,
                            metadata: t.spec.metadata.clone(),
                            created_at: now,
                        };
                        self.dead_letters.insert(Self::dl_key(&entry), entry);
                        TaskStatus::DeadLetter
                    }
                };
                let t = &self.tasks[task_id];
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(TaskStatus::Running),
                    to: Some(to),
                    attempt,
                    worker_id: Some(worker),
                    error: Some(error.clone()),
                });
            }

            WalRecord::LeaseExtended {
                task_id,
                run_id,
                lease_until,
            } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if let Some(run) = t.run_mut(run_id)
                    && run.status == RunStatus::Running
                {
                    run.lease_until = run.lease_until.max(*lease_until);
                    run.last_heartbeat = now;
                }
            }

            WalRecord::TaskCheckpointed {
                task_id,
                run_id,
                step,
                output,
            } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if t.status != TaskStatus::Running {
                    return out;
                }
                let Some(attempt_number) = t
                    .runs
                    .iter()
                    .find(|r| r.id == *run_id && r.status == RunStatus::Running)
                    .map(|r| r.attempt_number)
                else {
                    return out;
                };
                let checkpoint = CheckpointState {
                    step: step.clone(),
                    output: output.clone(),
                    run_id: run_id.clone(),
                    attempt_number,
                    created_at: now,
                };
                match t.checkpoints.iter_mut().find(|c| c.step == *step) {
                    Some(existing) => *existing = checkpoint,
                    None => t.checkpoints.push(checkpoint),
                }
                t.updated_at = now;
            }

            WalRecord::TaskPromoted { task_id } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                let from = t.status;
                let was_runnable = t.status == TaskStatus::Pending && t.due;
                match t.status {
                    TaskStatus::Retry | TaskStatus::Pending => {
                        t.status = TaskStatus::Pending;
                        t.due = true;
                        t.next_attempt_at = None;
                        t.updated_at = now;
                    }
                    _ => return out,
                }
                if !was_runnable {
                    out.push(Transition {
                        task_id: task_id.clone(),
                        queue_name: t.spec.queue_name.clone(),
                        from: Some(from),
                        to: Some(TaskStatus::Pending),
                        attempt: t.attempt_count,
                        worker_id: None,
                        error: None,
                    });
                }
            }

            WalRecord::TaskCancelled { task_id, reason } => {
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                if t.is_terminal() {
                    return out;
                }
                let from = t.status;
                let worker = t
                    .current_run()
                    .filter(|r| r.status == RunStatus::Running)
                    .map(|r| r.worker_id.clone());
                if let Some(run) = t.current_run_mut()
                    && run.status == RunStatus::Running
                {
                    run.status = RunStatus::Failed;
                    run.error_message = Some(reason.clone());
                    run.completed_at = Some(now);
                    run.ended_by = Some(RunEnd::Cancelled);
                }
                t.status = TaskStatus::Cancelled;
                t.error_message = Some(reason.clone());
                t.due = true;
                t.next_attempt_at = None;
                t.offered = false;
                t.updated_at = now;
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(from),
                    to: Some(TaskStatus::Cancelled),
                    attempt: t.attempt_count,
                    worker_id: worker,
                    error: None,
                });
            }

            WalRecord::TaskDeleted { task_id } => {
                let Some(t) = self.tasks.remove(task_id) else {
                    return out;
                };
                if let Some(k) = &t.spec.idempotency_key {
                    self.idempotency.remove(k);
                }
                for sid in &t.signals {
                    self.signals.remove(sid);
                }
                self.dead_letters.retain(|_, d| d.task_id != *task_id);
                out.push(Transition {
                    task_id: task_id.clone(),
                    queue_name: t.spec.queue_name.clone(),
                    from: Some(t.status),
                    to: None,
                    attempt: t.attempt_count,
                    worker_id: None,
                    error: None,
                });
            }

            WalRecord::SignalCreated {
                signal_id,
                task_id,
                signal_name,
                payload,
            } => {
                if self.signals.contains_key(signal_id) {
                    return out;
                }
                let Some(t) = self.tasks.get_mut(task_id) else {
                    return out;
                };
                t.signals.push(signal_id.clone());
                self.signals.insert(
                    signal_id.clone(),
                    SignalState {
                        id: signal_id.clone(),
                        task_id: task_id.clone(),
                        signal_name: signal_name.clone(),
                        payload: payload.clone(),
                        status: SignalStatus::Pending,
                        created_at: now,
                        delivered_at: None,
                        acknowledged_at: None,
                    },
                );
            }

            WalRecord::SignalDelivered { signal_id, .. } => {
                if let Some(s) = self.signals.get_mut(signal_id)
                    && s.status == SignalStatus::Pending
                {
                    s.status = SignalStatus::Delivered;
                    s.delivered_at = Some(now);
                }
            }

            WalRecord::SignalAcked { signal_id, .. } => {
                if let Some(s) = self.signals.get_mut(signal_id)
                    && s.status == SignalStatus::Delivered
                {
                    s.status = SignalStatus::Acknowledged;
                    s.acknowledged_at = Some(now);
                }
            }

            WalRecord::SignalsReset { task_id } => {
                for s in self.signals.values_mut() {
                    if s.task_id == *task_id && s.status == SignalStatus::Delivered {
                        s.status = SignalStatus::Pending;
                        s.delivered_at = None;
                    }
                }
            }

            WalRecord::ShardCleared => {
                for (_, t) in self.tasks.drain() {
                    out.push(Transition {
                        task_id: t.spec.id,
                        queue_name: t.spec.queue_name,
                        from: Some(t.status),
                        to: None,
                        attempt: t.attempt_count,
                        worker_id: None,
                        error: None,
                    });
                }
                self.signals.clear();
                self.idempotency.clear();
                self.dead_letters.clear();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use valka_core::{ShardId, TaskStatus};
    use valka_wal::{Envelope, FailureOutcome, Lsn, TaskSpec, WalRecord};

    use crate::state::{ShardSnapshot, ShardState, SignalStatus};

    fn spec(id: &str) -> TaskSpec {
        TaskSpec {
            id: id.into(),
            queue_name: "q".into(),
            task_name: "t".into(),
            input: None,
            priority: 0,
            max_retries: 3,
            timeout_seconds: 30,
            idempotency_key: None,
            metadata: serde_json::json!({}),
            scheduled_at: None,
            created_at: Utc::now(),
        }
    }

    fn env(rec: WalRecord) -> Envelope {
        Envelope::new(ShardId(0), rec)
    }

    #[test]
    fn full_lifecycle_and_idempotent_replay() {
        let mut s = ShardState::new(ShardId(0));
        let mut recs = [
            env(WalRecord::TaskCreated { task: spec("a") }),
            env(WalRecord::TaskDispatched {
                task_id: "a".into(),
                run_id: "r1".into(),
                attempt: 1,
                worker_id: "w".into(),
                node_id: "n".into(),
                lease_until: Utc::now(),
            }),
            env(WalRecord::RunCompleted {
                task_id: "a".into(),
                run_id: "r1".into(),
                output: Some(serde_json::json!({"ok": true})),
            }),
        ];
        let mut transitions = Vec::new();
        for r in recs.iter_mut() {
            transitions.extend(s.apply(r));
        }
        assert_eq!(s.shard_seq, 3);
        assert_eq!(s.tasks["a"].status, TaskStatus::Completed);
        assert_eq!(transitions.len(), 3);
        assert_eq!(transitions[2].to, Some(TaskStatus::Completed));

        // Replaying the same records (with their shard_seq set) is a no-op.
        let before = s.tasks["a"].clone();
        for r in recs.iter_mut() {
            assert!(s.apply(r).is_empty());
        }
        assert_eq!(s.tasks["a"], before);
        assert_eq!(s.shard_seq, 3);
    }

    #[test]
    fn retry_then_dead_letter() {
        let mut s = ShardState::new(ShardId(0));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("a") }));
        s.apply(&mut env(WalRecord::TaskDispatched {
            task_id: "a".into(),
            run_id: "r1".into(),
            attempt: 1,
            worker_id: "w".into(),
            node_id: "n".into(),
            lease_until: Utc::now(),
        }));
        let at = Utc::now() + chrono::Duration::seconds(5);
        let tr = s.apply(&mut env(WalRecord::RunFailed {
            task_id: "a".into(),
            run_id: "r1".into(),
            error: "boom".into(),
            outcome: FailureOutcome::Retry { at },
        }));
        assert_eq!(tr[0].to, Some(TaskStatus::Retry));
        assert!(!s.tasks["a"].due);
        assert_eq!(s.tasks["a"].next_attempt_at, Some(at));

        let tr = s.apply(&mut env(WalRecord::TaskPromoted {
            task_id: "a".into(),
        }));
        assert_eq!(tr[0].to, Some(TaskStatus::Pending));
        assert!(s.tasks["a"].is_runnable());

        s.apply(&mut env(WalRecord::TaskDispatched {
            task_id: "a".into(),
            run_id: "r2".into(),
            attempt: 2,
            worker_id: "w".into(),
            node_id: "n".into(),
            lease_until: Utc::now(),
        }));
        s.apply(&mut env(WalRecord::LeaseExpired {
            task_id: "a".into(),
            run_id: "r2".into(),
            error: "lease expired".into(),
            outcome: FailureOutcome::DeadLetter,
        }));
        assert_eq!(s.tasks["a"].status, TaskStatus::DeadLetter);
        assert_eq!(s.dead_letters.len(), 1);
        assert_eq!(s.tasks["a"].runs.len(), 2);

        // Deleting the task removes its DLQ entry.
        s.apply(&mut env(WalRecord::TaskDeleted {
            task_id: "a".into(),
        }));
        assert!(s.tasks.is_empty() && s.dead_letters.is_empty());
    }

    #[test]
    fn stale_completion_after_cancel_is_noop() {
        let mut s = ShardState::new(ShardId(0));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("a") }));
        s.apply(&mut env(WalRecord::TaskDispatched {
            task_id: "a".into(),
            run_id: "r1".into(),
            attempt: 1,
            worker_id: "w".into(),
            node_id: "n".into(),
            lease_until: Utc::now(),
        }));
        let tr = s.apply(&mut env(WalRecord::TaskCancelled {
            task_id: "a".into(),
            reason: "user".into(),
        }));
        assert_eq!(tr[0].worker_id.as_deref(), Some("w"));
        assert!(
            s.apply(&mut env(WalRecord::RunCompleted {
                task_id: "a".into(),
                run_id: "r1".into(),
                output: None,
            }))
            .is_empty()
        );
        assert_eq!(s.tasks["a"].status, TaskStatus::Cancelled);
    }

    #[test]
    fn idempotency_key_dedupes_within_shard() {
        let mut s = ShardState::new(ShardId(0));
        let mut a = spec("a");
        a.idempotency_key = Some("k".into());
        let mut b = spec("b");
        b.idempotency_key = Some("k".into());
        assert_eq!(
            s.apply(&mut env(WalRecord::TaskCreated { task: a })).len(),
            1
        );
        assert!(
            s.apply(&mut env(WalRecord::TaskCreated { task: b }))
                .is_empty()
        );
        assert_eq!(s.tasks.len(), 1);
    }

    #[test]
    fn snapshot_round_trip_preserves_state() {
        let mut s = ShardState::new(ShardId(3));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("a") }));
        s.apply(&mut env(WalRecord::SignalCreated {
            signal_id: "sig".into(),
            task_id: "a".into(),
            signal_name: "ping".into(),
            payload: None,
        }));
        let snap = s.to_snapshot(Utc::now());
        let json = serde_json::to_string(&snap).unwrap();
        let back: ShardSnapshot = serde_json::from_str(&json).unwrap();
        let r = ShardState::from_snapshot(back, Lsn::new(1, 4));
        assert_eq!(r.shard_seq, 2);
        assert_eq!(r.tasks["a"].signals, vec!["sig".to_string()]);
        assert_eq!(r.signals["sig"].status, SignalStatus::Pending);
        assert_eq!(r.snapshot_lsn, Lsn::new(1, 4));
    }

    fn dispatched(s: &mut ShardState, task: &str, run: &str, attempt: i32) {
        s.apply(&mut env(WalRecord::TaskDispatched {
            task_id: task.into(),
            run_id: run.into(),
            attempt,
            worker_id: "w".into(),
            node_id: "n".into(),
            lease_until: Utc::now(),
        }));
    }

    fn checkpointed(task: &str, run: &str, step: &str, output: serde_json::Value) -> Envelope {
        env(WalRecord::TaskCheckpointed {
            task_id: task.into(),
            run_id: run.into(),
            step: step.into(),
            output,
        })
    }

    #[test]
    fn checkpoint_applies_only_to_the_running_run() {
        let mut s = ShardState::new(ShardId(0));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("a") }));
        s.apply(&mut checkpointed("a", "r1", "early", serde_json::json!(0)));
        assert!(
            s.tasks["a"].checkpoints.is_empty(),
            "pending task has no run"
        );

        dispatched(&mut s, "a", "r1", 1);
        s.apply(&mut checkpointed("a", "r1", "fetch", serde_json::json!(1)));
        s.apply(&mut checkpointed("a", "r1", "parse", serde_json::json!(2)));
        s.apply(&mut checkpointed(
            "a",
            "other",
            "rogue",
            serde_json::json!(3),
        ));
        s.apply(&mut checkpointed(
            "missing",
            "r1",
            "x",
            serde_json::json!(4),
        ));
        let steps: Vec<&str> = s.tasks["a"]
            .checkpoints
            .iter()
            .map(|c| c.step.as_str())
            .collect();
        assert_eq!(steps, vec!["fetch", "parse"]);

        s.apply(&mut env(WalRecord::RunFailed {
            task_id: "a".into(),
            run_id: "r1".into(),
            error: "boom".into(),
            outcome: FailureOutcome::Retry { at: Utc::now() },
        }));
        s.apply(&mut checkpointed("a", "r1", "late", serde_json::json!(5)));
        s.apply(&mut env(WalRecord::TaskPromoted {
            task_id: "a".into(),
        }));
        dispatched(&mut s, "a", "r2", 2);
        s.apply(&mut checkpointed("a", "r2", "fetch", serde_json::json!(10)));

        let cps = &s.tasks["a"].checkpoints;
        assert_eq!(cps.len(), 2, "the failed run's late checkpoint is a no-op");
        assert_eq!(cps[0].step, "fetch");
        assert_eq!(cps[0].output, serde_json::json!(10));
        assert_eq!(cps[0].attempt_number, 2);
        assert_eq!(cps[0].run_id, "r2");
        assert_eq!(cps[1].attempt_number, 1);
    }

    #[test]
    fn checkpoints_round_trip_through_snapshots_and_old_snapshots_load() {
        let mut s = ShardState::new(ShardId(1));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("a") }));
        dispatched(&mut s, "a", "r1", 1);
        s.apply(&mut checkpointed(
            "a",
            "r1",
            "fetch",
            serde_json::json!({"k": 1}),
        ));
        let json = serde_json::to_value(s.to_snapshot(Utc::now())).unwrap();
        let back: ShardSnapshot = serde_json::from_value(json.clone()).unwrap();
        let r = ShardState::from_snapshot(back, Lsn::new(1, 1));
        assert_eq!(r.tasks["a"].checkpoints, s.tasks["a"].checkpoints);

        let mut old = json;
        old["tasks"][0]
            .as_object_mut()
            .unwrap()
            .remove("checkpoints");
        let back: ShardSnapshot = serde_json::from_value(old).unwrap();
        assert!(back.tasks[0].checkpoints.is_empty());
    }

    #[test]
    fn eviction_drops_only_old_terminal_tasks() {
        let mut s = ShardState::new(ShardId(0));
        s.apply(&mut env(WalRecord::TaskCreated { task: spec("live") }));
        let mut done = env(WalRecord::TaskCreated { task: spec("done") });
        done.ts_ms = 1000;
        s.apply(&mut done);
        let mut c = env(WalRecord::TaskCancelled {
            task_id: "done".into(),
            reason: "x".into(),
        });
        c.ts_ms = 2000;
        s.apply(&mut c);
        assert_eq!(s.evict_terminal_before(Utc::now()), 1);
        assert!(s.tasks.contains_key("live"));
    }
}
