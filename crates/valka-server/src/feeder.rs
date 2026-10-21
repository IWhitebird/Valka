//! Remote feeding: this node's idle workers take tasks owned by other nodes.
//!
//! While some local worker waits for a queue, one fetch loop runs per (queue, owner node)
//! for the owners of that queue's partitions. A fetch long-polls the owner, which hands
//! out runnable tasks reserved for this node (in RAM only). Each fetched task is offered
//! to a waiting local worker; the worker's dispatch then goes to the owner, which records
//! it (`Command::Dispatch`). A task no local worker takes goes straight back
//! (`Command::Unoffer`); an unused reservation lapses on the owner after 30 s anyway.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;
use tracing::debug;
use valka_cluster::{Command, Reply};
use valka_dispatcher::TaskRouter;
use valka_engine::{OfferOutcome, TaskSink};
use valka_matching::MatchingSink;

use crate::api::Api;

/// How long one fetch waits at the owner for a task to appear.
const FETCH_WAIT: Duration = Duration::from_secs(2);

pub fn spawn_remote_feeder(api: Api, mut shutdown: watch::Receiver<bool>) {
    let active: Arc<Mutex<HashSet<(String, String)>>> = Arc::new(Mutex::new(HashSet::new()));
    tokio::spawn(async move {
        let matching = api.dispatcher().matching().clone();
        let waiting = matching.waiting_notify();
        loop {
            tokio::select! {
                r = shutdown.changed() => { if r.is_err() || *shutdown.borrow() { return; } }
                _ = waiting.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
            if !api.engine().is_serving() {
                continue;
            }
            for queue in matching.queues_with_waiters() {
                let owners = api.remote_owners(&queue).await;
                // The free slots are shared by this node's feeder and one fetch per owner.
                let sharers = owners.len() + 1;
                for node in owners {
                    let key = (queue.clone(), node.clone());
                    if !active.lock().insert(key.clone()) {
                        continue;
                    }
                    let (api, active) = (api.clone(), active.clone());
                    tokio::spawn(async move {
                        fetch_loop(&api, &key.0, &key.1, sharers).await;
                        active.lock().remove(&key);
                    });
                }
            }
        }
    });
}

/// Fetch tasks of `queue` from `node` while local workers wait for them. Each fetch asks
/// for this loop's share of the free slots: taking all of them from every owner would
/// fetch tasks no worker can take, each sent back with a call of its own.
async fn fetch_loop(api: &Api, queue: &str, node: &str, sharers: usize) {
    let matching = api.dispatcher().matching().clone();
    let sink = MatchingSink::new(matching.clone());
    loop {
        let waiting = matching.waiting(queue);
        if waiting == 0 || !api.engine().is_serving() {
            return;
        }
        let reply = api
            .send_to_node(
                node,
                Command::Fetch {
                    queue: queue.to_string(),
                    max: waiting.div_ceil(sharers.max(1)),
                    wait_ms: FETCH_WAIT.as_millis() as u64,
                },
            )
            .await;
        let tasks = match reply {
            Some(Reply::Fetched(t)) => {
                valka_core::metrics::record_remote_fetch(t.len());
                t
            }
            // Unreachable or not an owner any more: the feeder re-plans on its next tick.
            _ => return,
        };
        let mut refused = Vec::new();
        for task in tasks {
            let id = task.task_id.clone();
            if sink.offer(task) != OfferOutcome::Matched {
                debug!(task_id = %id, node, "no local worker took a fetched task; returning it");
                refused.push(id);
            }
        }
        futures::future::join_all(refused.iter().map(|id| api.unoffer(id))).await;
    }
}
