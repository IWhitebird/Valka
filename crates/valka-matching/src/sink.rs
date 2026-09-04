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
    fn offer(&self, task: DispatchableTask) -> OfferOutcome {
        let queue = task.queue_name.clone();
        let partition = self.partition(&queue, &task.task_id);
        match self
            .matching
            .offer_task(&queue, partition, Self::envelope(task))
        {
            Ok(()) => OfferOutcome::Matched,
            Err(envelope) => {
                if self.matching.buffer_task(&queue, partition, envelope) {
                    OfferOutcome::Buffered
                } else {
                    OfferOutcome::Rejected
                }
            }
        }
    }

    fn capacity(&self, queue_name: &str) -> usize {
        self.matching.free_capacity(queue_name)
    }
}
