//! Adapter: the engine hands runnable tasks to the matching service through this.

use crate::partition::TaskEnvelope;
use crate::service::MatchingService;
use valka_core::{PartitionId, partition_for_task};
use valka_engine::{DispatchableTask, OfferOutcome, TaskSink};

pub struct MatchingSink {
    matching: MatchingService,
}

impl MatchingSink {
    pub fn new(matching: MatchingService) -> Self {
        Self { matching }
    }

    fn partition(&self, queue: &str, task_id: &str) -> PartitionId {
        partition_for_task(queue, task_id, self.matching.config().num_partitions)
    }

    pub fn envelope(task: DispatchableTask) -> TaskEnvelope {
        TaskEnvelope {
            task_id: task.task_id,
            task_run_id: String::new(),
            queue_name: task.queue_name,
            task_name: task.task_name,
            input: task.input.map(|v| v.to_string()),
            attempt_number: task.attempt_number,
            timeout_seconds: task.timeout_seconds,
            metadata: task.metadata.to_string(),
            priority: task.priority,
        }
    }
}

impl TaskSink for MatchingSink {
    /// Match with a waiting worker or reject. Tasks are not buffered here: a runnable task
    /// waits in its owner's pending index, where this node's feeder and other nodes'
    /// fetches can both take it.
    fn offer(&self, task: DispatchableTask) -> OfferOutcome {
        let queue = task.queue_name.clone();
        let partition = self.partition(&queue, &task.task_id);
        match self
            .matching
            .offer_task(&queue, partition, Self::envelope(task))
        {
            Ok(()) => OfferOutcome::Matched,
            Err(_) => OfferOutcome::Rejected,
        }
    }

    fn capacity(&self, queue_name: &str) -> usize {
        self.matching.waiting(queue_name)
    }
}
