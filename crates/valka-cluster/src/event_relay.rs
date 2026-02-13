use std::sync::Arc;

use tokio::sync::{broadcast, watch};
use tracing::{debug, warn};

use crate::forwarder::NodeForwarder;
use crate::gossip::ClusterManager;

/// Background task that relays locally-originated events to all peer nodes.
/// Only events with `node_id == self_node_id` or empty node_id are relayed,
/// preventing relay loops (forwarded events carry the originating node's ID).
pub async fn run_event_relay(
    cluster: Arc<ClusterManager>,
    forwarder: NodeForwarder,
    mut event_rx: broadcast::Receiver<valka_proto::TaskEvent>,
    mut shutdown: watch::Receiver<bool>,
) {
    let self_node_id = cluster.node_id().0.clone();

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("Event relay shutting down");
                    return;
                }
            }
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Only relay events that originated locally
                        if !event.node_id.is_empty() && event.node_id != self_node_id {
                            continue;
                        }

                        let members = cluster.members().await;
                        for member in &members {
                            if member == &self_node_id {
                                continue;
                            }
                            if let Some(addr) = cluster.get_node_grpc_addr(member).await {
                                let forwarder = forwarder.clone();
                                let event = event.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = forwarder.forward_event(&addr, event).await {
                                        debug!(
                                            peer = %addr,
                                            error = %e,
                                            "Failed to relay event to peer"
                                        );
                                    }
                                });
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "Event relay lagged, missed events");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("Event relay channel closed");
                        return;
                    }
                }
            }
        }
    }
}
