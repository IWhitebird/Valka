use valka_core::NodeId;

/// Events emitted by the cluster manager
#[derive(Debug, Clone)]
pub enum ClusterEvent {
    NodeJoined { node_id: NodeId, grpc_addr: String },
    NodeLeft { node_id: NodeId },
    PartitionsRebalanced,
}
