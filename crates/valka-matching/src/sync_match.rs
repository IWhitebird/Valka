use crate::partition::TaskEnvelope;
use crate::service::MatchingService;
use tracing::debug;
use valka_core::PartitionId;

/// Attempt a synchronous match for a task.
/// Returns Ok(()) if matched, Err(task) if no worker available.
pub fn try_sync_match(
    service: &MatchingService,
    queue_name: &str,
    partition_id: PartitionId,
    task: TaskEnvelope,
) -> Result<(), TaskEnvelope> {
    // Try the direct partition first
    let task = {
        if let Some(mut partition) = service.get_partition_mut(queue_name, partition_id) {
            match partition.try_match_task(task) {
                None => {
                    debug!(
                        queue = queue_name,
                        partition = partition_id.0,
                        "Sync match: task matched on direct partition"
                    );
                    valka_core::metrics::record_sync_match();
                    return Ok(());
                }
                Some(task) => task, // Drop the DashMap guard before forwarding
            }
        } else {
            task
        }
    };

    // Guard is dropped here - safe to forward up the tree
    try_forward_up(service, queue_name, partition_id, task)
}

/// Forward a task up the partition tree looking for available workers
fn try_forward_up(
    service: &MatchingService,
    queue_name: &str,
    from_partition: PartitionId,
    task: TaskEnvelope,
) -> Result<(), TaskEnvelope> {
    // Read the parent outside of any mutable borrow
    let parent = service
        .get_partition(queue_name, from_partition)
        .and_then(|p| p.parent);

    if let Some(parent_id) = parent {
        let task = {
            if let Some(mut parent_partition) = service.get_partition_mut(queue_name, parent_id) {
                match parent_partition.try_match_task(task) {
                    None => {
                        debug!(
                            queue = queue_name,
                            partition = parent_id.0,
                            "Sync match: task matched via tree forwarding"
                        );
                        valka_core::metrics::record_sync_match();
                        return Ok(());
                    }
                    Some(task) => task, // Drop the guard
                }
            } else {
                task
            }
        };

        // Guard dropped, recurse
        return try_forward_up(service, queue_name, parent_id, task);
    }

    Err(task)
}
