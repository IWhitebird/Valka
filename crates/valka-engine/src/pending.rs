//! The runnable-task index and the hand-off to the matching layer.

use std::collections::BTreeSet;
use std::time::Duration;
use tracing::debug;
use valka_core::TaskStatus;

use crate::engine::{Engine, PendingKey};
use crate::sink::{DispatchableTask, OfferOutcome};
use crate::state::Transition;
use crate::timers::TimerKind;
use crate::write_path::{dispatchable, pending_key};

/// How long an offered task (taken from the pending index for this node's matching layer,
/// or reserved for another node by `fetch`) may stay undispatched before it goes back to
/// the pending index. A safety net: whatever loses an offer (a fetcher that died, a
/// dispatch that failed) cannot strand the task.
pub const OFFER_TTL: Duration = Duration::from_secs(30);

impl Engine {
    pub fn queues(&self) -> Vec<String> {
        let mut qs: BTreeSet<String> = BTreeSet::new();
        for m in &self.inner.shards {
            let st = m.lock();
            if st.serving() {
                for t in st.tasks.values() {
                    qs.insert(t.spec.queue_name.clone());
                }
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
            let Some(shard) = self.shard_of(&task_id) else {
                continue;
            };
            let mut st = self.inner.shards[shard.index()].lock();
            let Some(t) = st.tasks.get_mut(&task_id) else {
                continue;
            };
            if !t.is_runnable() {
                continue;
            }
            t.offered = true;
            out.push(dispatchable(t, t.attempt_count + 1));
        }
        self.note_offered(out.iter().map(|t| t.task_id.clone()));
        out
    }

    /// Start the `OFFER_TTL` clock of tasks just marked offered.
    fn note_offered(&self, task_ids: impl IntoIterator<Item = String>) {
        let lapse = tokio::time::Instant::now() + OFFER_TTL;
        let mut offers = self.inner.offers.lock();
        for id in task_ids {
            offers.insert(id, lapse);
        }
    }

    /// The matching layer dropped a task it had been offered; make it runnable again.
    pub fn unoffer(&self, task_id: &str) {
        let Some(shard) = self.shard_of(task_id) else {
            return;
        };
        let key = {
            let mut st = self.inner.shards[shard.index()].lock();
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
        self.inner.offers.lock().remove(task_id);
        if let Some(entry) = key {
            self.add_runnable([entry]);
        }
    }

    /// Put runnable tasks in the pending index and wake whoever hands them out: the local
    /// feeder and other nodes' fetches.
    pub(crate) fn add_runnable(&self, entries: impl IntoIterator<Item = (String, PendingKey)>) {
        let mut added = false;
        {
            let mut p = self.inner.pending.lock();
            for (q, k) in entries {
                p.entry(q).or_default().insert(k);
                added = true;
            }
        }
        if added {
            self.inner.pending_wake.notify_one();
            self.inner.runnable.notify_waiters();
        }
    }

    /// Hand up to `max` runnable tasks of `queue` to another node, waiting up to `wait`
    /// for one to appear (long poll). The tasks stay PENDING, reserved for the caller in
    /// RAM only: it dispatches each to one of its workers with
    /// [`Engine::dispatch_to`], or gives it back with [`Engine::unoffer`]; a reservation
    /// not used within `OFFER_TTL` lapses.
    pub async fn fetch(&self, queue: &str, max: usize, wait: Duration) -> Vec<DispatchableTask> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.inner.runnable.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let tasks = if self.is_serving() {
                self.take_pending(queue, max)
            } else {
                Vec::new()
            };
            if !tasks.is_empty() || tokio::time::Instant::now() >= deadline {
                return tasks;
            }
            tokio::select! {
                _ = &mut notified => {}
                _ = tokio::time::sleep_until(deadline) => {}
            }
        }
    }

    /// Offers never dispatched within `OFFER_TTL`: make the tasks runnable again.
    pub(crate) fn lapse_offers(&self) {
        let now = tokio::time::Instant::now();
        let lapsed: Vec<String> = {
            let mut offers = self.inner.offers.lock();
            let lapsed: Vec<String> = offers
                .iter()
                .filter(|(_, at)| **at <= now)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &lapsed {
                offers.remove(id);
            }
            lapsed
        };
        for id in lapsed {
            debug!(task_id = %id, "offer lapsed undispatched");
            self.unoffer(&id);
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
        let Some(shard) = self.shard_of(task_id) else {
            return;
        };
        let (dispatchable, promote_at, key, queue) = {
            let mut st = self.inner.shards[shard.index()].lock();
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
            OfferOutcome::Matched | OfferOutcome::Buffered => self.note_offered([task_id.into()]),
            OfferOutcome::Rejected => {
                if let Some(t) = self.inner.shards[shard.index()]
                    .lock()
                    .tasks
                    .get_mut(task_id)
                {
                    t.offered = false;
                }
                if let Some(k) = key {
                    self.add_runnable([(queue, k)]);
                }
            }
        }
    }
}
