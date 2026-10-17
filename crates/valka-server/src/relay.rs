//! Cluster-wide event stream. Each node broadcasts the events of the shards it serves on
//! its local channel. While this node has subscribers, it also streams every live peer's
//! local events (`InternalService.Events`) into the cluster-wide channel, so a subscriber
//! sees the whole cluster whichever node it is connected to.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tracing::debug;
use valka_proto::TaskEvent;

use crate::api::Api;

pub fn spawn_event_relay(
    api: Api,
    local: broadcast::Sender<TaskEvent>,
    all: broadcast::Sender<TaskEvent>,
    mut shutdown: watch::Receiver<bool>,
) {
    // This node's own events.
    {
        let (mut rx, all) = (local.subscribe(), all.clone());
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let _ = all.send(ev);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }
    // Peers' events, only while someone listens.
    tokio::spawn(async move {
        let mut streams: HashMap<String, JoinHandle<()>> = HashMap::new();
        loop {
            tokio::select! {
                r = shutdown.changed() => {
                    if r.is_err() || *shutdown.borrow() {
                        for (_, h) in streams.drain() { h.abort(); }
                        return;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
            streams.retain(|_, h| !h.is_finished());
            let want: HashMap<String, String> = if all.receiver_count() > 0 {
                api.engine()
                    .live_peers()
                    .into_iter()
                    .filter_map(|(n, l)| l.internal_addr.map(|a| (n, a)))
                    .collect()
            } else {
                HashMap::new()
            };
            streams.retain(|node, h| {
                let keep = want.contains_key(node);
                if !keep {
                    h.abort();
                }
                keep
            });
            for (node, addr) in want {
                if streams.contains_key(&node) {
                    continue;
                }
                let (peers, all, me) =
                    (api.peers().clone(), all.clone(), api.node_id().to_string());
                streams.insert(
                    node.clone(),
                    tokio::spawn(async move {
                        match peers.events(&addr, &me).await {
                            Ok(mut stream) => {
                                while let Some(Ok(ev)) = stream.next().await {
                                    let _ = all.send(ev);
                                }
                            }
                            Err(e) => debug!(node, error = %e, "peer event stream failed"),
                        }
                    }),
                );
            }
        }
    });
}
