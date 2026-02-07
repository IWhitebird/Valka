use valka_core::NodeId;

/// Events emitted by the cluster manager
#[derive(Debug, Clone)]
pub enum ClusterEvent {
    NodeJoined(NodeId),
    NodeLeft(NodeId),
    PartitionsRebalanced,
}
