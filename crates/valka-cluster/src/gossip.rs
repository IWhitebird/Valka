use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use valka_core::{GossipConfig, NodeId};

use crate::ring::HashRing;

/// Manages cluster membership via chitchat gossip protocol.
/// In single-node mode (Phase 1), this is a no-op that owns all partitions.
pub struct ClusterManager {
    node_id: NodeId,
    ring: Arc<RwLock<HashRing>>,
    members: Arc<RwLock<HashSet<String>>>,
    _config: GossipConfig,
}

impl ClusterManager {
    pub fn new_single_node(node_id: NodeId, _num_partitions: i32) -> Self {
        let mut ring = HashRing::new();
        ring.add_node(&node_id.0);

        let mut members = HashSet::new();
        members.insert(node_id.0.clone());

        Self {
            node_id,
            ring: Arc::new(RwLock::new(ring)),
            members: Arc::new(RwLock::new(members)),
            _config: GossipConfig::default(),
        }
    }

    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    /// Check if this node owns the given partition
    pub async fn owns_partition(&self, queue_name: &str, partition_id: i32) -> bool {
        let ring = self.ring.read().await;
        let key = format!("{queue_name}:{partition_id}");
        ring.get_node(&key)
            .map(|n| n == self.node_id.0)
            .unwrap_or(true) // Single-node: always own
    }

    /// Get all members
    pub async fn members(&self) -> HashSet<String> {
        self.members.read().await.clone()
    }
}
