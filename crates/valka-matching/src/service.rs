use crate::partition::{PartitionQueue, TaskEnvelope, WorkerSlot};
use crate::sync_match;
use dashmap::DashMap;
use dashmap::mapref::one::{Ref, RefMut};
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::{debug, info};
use valka_core::{MatchingConfig, PartitionId, WorkerId};

/// Composite key for partition lookup: (queue_name, partition_id)
type PartitionKey = (String, i32);

/// The core matching service that routes tasks to workers.
#[derive(Clone)]
pub struct MatchingService {
    partitions: Arc<DashMap<PartitionKey, PartitionQueue>>,
    config: MatchingConfig,
}

impl MatchingService {
    pub fn new(config: MatchingConfig) -> Self {
        Self {
            partitions: Arc::new(DashMap::new()),
            config,
        }
    }

    /// Ensure partitions exist for a queue, building the partition tree.
    pub fn ensure_queue(&self, queue_name: &str) {
        let n = self.config.num_partitions;
        let bf = self.config.branching_factor;

        for i in 0..n {
            let key = (queue_name.to_string(), i);
            if self.partitions.contains_key(&key) {
                continue;
            }
            let parent = if i == 0 {
                None
            } else {
                Some(PartitionId((i - 1) / bf as i32))
            };
            let mut pq = PartitionQueue::new(
                PartitionId(i),
                queue_name.to_string(),
                parent,
                self.config.max_buffer_per_partition,
            );

            // Set children
            for c in 1..=bf {
                let child_id = i * bf as i32 + c as i32;
                if child_id < n {
                    pq.children.push(PartitionId(child_id));
                }
            }

            self.partitions.insert(key, pq);
        }
    }

    pub fn get_partition(
        &self,
        queue_name: &str,
        partition_id: PartitionId,
    ) -> Option<Ref<'_, PartitionKey, PartitionQueue>> {
        self.partitions
            .get(&(queue_name.to_string(), partition_id.0))
    }

    pub fn get_partition_mut(
        &self,
        queue_name: &str,
        partition_id: PartitionId,
    ) -> Option<RefMut<'_, PartitionKey, PartitionQueue>> {
        self.partitions
            .get_mut(&(queue_name.to_string(), partition_id.0))
    }

    /// Offer a task for sync matching. Returns the task back if no match.
    pub fn offer_task(
        &self,
        queue_name: &str,
        partition_id: PartitionId,
        task: TaskEnvelope,
    ) -> Result<(), TaskEnvelope> {
        self.ensure_queue(queue_name);
        sync_match::try_sync_match(self, queue_name, partition_id, task)
    }

    /// Register a worker as waiting for a task on a given queue/partition.
    /// Returns a oneshot receiver that will receive the task assignment.
    pub fn register_worker(
        &self,
        queue_name: &str,
        partition_id: PartitionId,
        worker_id: WorkerId,
    ) -> oneshot::Receiver<TaskEnvelope> {
        self.ensure_queue(queue_name);

        let (tx, rx) = oneshot::channel();
        let slot = WorkerSlot {
            worker_id: worker_id.clone(),
            task_sender: tx,
        };

        if let Some(mut partition) = self.get_partition_mut(queue_name, partition_id) {
            let matched = partition.register_worker(slot);
            if matched {
                debug!(
                    queue = queue_name,
                    partition = partition_id.0,
                    worker = %worker_id,
                    "Worker immediately matched with pending task"
                );
            }
        }

        rx
    }

    /// Deregister a worker from all partitions (e.g., on disconnect)
    pub fn deregister_worker(&self, worker_id: &WorkerId) {
        for mut entry in self.partitions.iter_mut() {
            entry
                .value_mut()
                .waiting_workers
                .retain(|slot| slot.worker_id != *worker_id);
        }
        info!(worker = %worker_id, "Worker deregistered from matching service");
    }

    /// Buffer a task that wasn't matched (for TaskReader path)
    pub fn buffer_task(
        &self,
        queue_name: &str,
        partition_id: PartitionId,
        task: TaskEnvelope,
    ) -> bool {
        self.ensure_queue(queue_name);
        if let Some(mut partition) = self.get_partition_mut(queue_name, partition_id) {
            partition.buffer_task(task)
        } else {
            false
        }
    }

    pub fn config(&self) -> &MatchingConfig {
        &self.config
    }
}
