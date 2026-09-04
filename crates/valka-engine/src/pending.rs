//! The runnable-task index and the hand-off to the matching layer.

use std::collections::BTreeSet;
use valka_core::{TaskStatus, shard_of_task_id};

use crate::engine::{Engine, PendingKey};
use crate::sink::{DispatchableTask, OfferOutcome};
use crate::state::Transition;
use crate::timers::TimerKind;
use crate::write_path::{dispatchable, pending_key};

impl Engine {
    pub fn queues(&self) -> Vec<String> {
        let mut qs: BTreeSet<String> = BTreeSet::new();
        for m in &self.inner.shards {
            for t in m.lock().tasks.values() {
                qs.insert(t.spec.queue_name.clone());
            }
        }
        qs.into_iter().collect()
    }

    pub fn pending_count(&self, queue: &str) -> usize {
        self.inner.pending.lock().get(queue).map_or(0, |s| s.len())
    }

    /// Pull up to `max` runnable tasks for a queue, marking them offered.
    pub fn take_pending(&self, queue: &str, max: usize) -> Vec<DispatchableTask> {
        if max == 0 {
            return Vec::new();
        }
        let keys: Vec<PendingKey> = {
            let mut p = self.inner.pending.lock();
            let Some(set) = p.get_mut(queue) else {
                return Vec::new();
            };
            let mut out = Vec::with_capacity(max.min(set.len()));
            while out.len() < max {
                let Some(k) = set.pop_first() else { break };
                out.push(k);
            }
            if set.is_empty() {
                p.remove(queue);
            }
            out
        };
        let mut out = Vec::with_capacity(keys.len());
        for (_, _, task_id) in keys {
            let Some(shard) = shard_of_task_id(&task_id) else {
                continue;
            };
            let mut st = self.inner.shards[shard.0 as usize].lock();
            let Some(t) = st.tasks.get_mut(&task_id) else {
                continue;
            };
            if !t.is_runnable() {
                continue;
            }
            t.offered = true;
            out.push(dispatchable(t, t.attempt_count + 1));
        }
        out
    }

    /// The matching layer dropped a task it had been offered; make it runnable again.
    pub fn unoffer(&self, task_id: &str) {
        let Some(shard) = shard_of_task_id(task_id) else {
            return;
        };
        let key = {
            let mut st = self.inner.shards[shard.0 as usize].lock();
            let Some(t) = st.tasks.get_mut(task_id) else {
                return;
            };
            t.offered = false;
            if t.is_runnable() {
                Some((t.spec.queue_name.clone(), pending_key(t)))
            } else {
                None
            }
        };
        if let Some((q, k)) = key {
            self.inner.pending.lock().entry(q).or_default().insert(k);
            self.inner.pending_wake.notify_one();
        }
    }

    pub(crate) fn drop_from_pending(&self, transitions: &[Transition]) {
        let leaving: Vec<&Transition> = transitions
            .iter()
            .filter(|t| t.from == Some(TaskStatus::Pending) && t.to != Some(TaskStatus::Pending))
            .collect();
        if leaving.is_empty() {
            return;
        }
        let mut p = self.inner.pending.lock();
        for t in leaving {
            if let Some(set) = p.get_mut(&t.queue_name) {
                set.retain(|(_, _, id)| *id != t.task_id);
                if set.is_empty() {
                    p.remove(&t.queue_name);
                }
            }
        }
    }

    /// A task just became PENDING: if due, try the sink (hot path); otherwise arm its
    /// promote timer.
    pub(crate) fn index_or_offer(&self, task_id: &str) {
        let Some(shard) = shard_of_task_id(task_id) else {
            return;
        };
        let (dispatchable, promote_at, key, queue) = {
            let mut st = self.inner.shards[shard.0 as usize].lock();
            let Some(t) = st.tasks.get_mut(task_id) else {
                return;
            };
            if t.status != TaskStatus::Pending {
                return;
            }
            if !t.due {
                (None, t.next_attempt_at, None, t.spec.queue_name.clone())
            } else if t.offered {
                return;
            } else {
                t.offered = true;
                (
                    Some(dispatchable(t, t.attempt_count + 1)),
                    None,
                    Some(pending_key(t)),
                    t.spec.queue_name.clone(),
                )
            }
        };
        if let Some(at) = promote_at {
            self.inner.timers.lock().schedule(
                at,
                TimerKind::Promote {
                    task_id: task_id.into(),
                },
            );
            return;
        }
        let Some(d) = dispatchable else { return };
        let sink = self.inner.sink.read().clone();
        match sink.offer(d) {
            OfferOutcome::Matched | OfferOutcome::Buffered => {}
            OfferOutcome::Rejected => {
                if let Some(t) = self.inner.shards[shard.0 as usize]
                    .lock()
                    .tasks
                    .get_mut(task_id)
                {
                    t.offered = false;
                }
                if let Some(k) = key {
                    self.inner
                        .pending
                        .lock()
                        .entry(queue)
                        .or_default()
                        .insert(k);
                    self.inner.pending_wake.notify_one();
                }
            }
        }
    }
}
