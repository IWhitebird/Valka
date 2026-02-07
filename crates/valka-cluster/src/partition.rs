use valka_core::PartitionId;

/// Partition assignment information
#[derive(Debug, Clone)]
pub struct PartitionAssignment {
    pub partition_id: PartitionId,
    pub queue_name: String,
    pub owner_node_id: String,
}
