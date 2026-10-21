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
    /// Woken whenever a worker starts waiting (feeders and remote fetchers listen).
    waiting_changed: Arc<tokio::sync::Notify>,
}

impl MatchingService {
    pub fn new(config: MatchingConfig) -> Self {
        Self {
            partitions: Arc::new(DashMap::new()),
            config,
            waiting_changed: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Notified whenever a worker registers as waiting.
    pub fn waiting_notify(&self) -> Arc<tokio::sync::Notify> {
        self.waiting_changed.clone()
    }

    /// Tasks of `queue` the waiting workers can take right now: one per open slot. A
    /// worker registers one slot per free concurrency slot (up to a batch), so this is
    /// what a feeder may offer without a rejection.
    pub fn waiting(&self, queue_name: &str) -> usize {
        (0..self.config.num_partitions)
            .filter_map(|i| self.partitions.get(&(queue_name.to_string(), i)))
            .map(|p| {
                p.waiting_workers
                    .iter()
                    .filter(|slot| !slot.task_sender.is_closed())
                    .count()
            })
            .sum()
    }

    /// Queues with at least one waiting worker.
    pub fn queues_with_waiters(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .partitions
            .iter()
            .filter(|e| {
                e.value()
                    .waiting_workers
                    .iter()
                    .any(|s| !s.task_sender.is_closed())
            })
            .map(|e| e.key().0.clone())
            .collect();
        out.sort();
        out.dedup();
        out
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
    #[allow(clippy::result_large_err)]
    pub fn offer_task(
        &self,
        queue_name: &str,
        partition_id: PartitionId,
        task: TaskEnvelope,
    ) -> Result<(), TaskEnvelope> {
        self.ensure_queue(queue_name);
        let task = match sync_match::try_sync_match(self, queue_name, partition_id, task) {
            Ok(()) => return Ok(()),
            Err(task) => task,
        };
        // Any slot of the queue will do: a worker of the queue runs tasks of every
        // partition, and its slots may all sit on other partitions.
        let mut task = task;
        for i in 0..self.config.num_partitions {
            if i == partition_id.0 {
                continue;
            }
            let Some(mut p) = self.get_partition_mut(queue_name, PartitionId(i)) else {
                continue;
            };
            match p.try_match_task(task) {
                None => {
                    valka_core::metrics::record_sync_match();
                    return Ok(());
                }
                Some(t) => task = t,
            }
        }
        Err(task)
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

        let matched = match self.get_partition_mut(queue_name, partition_id) {
            Some(mut partition) => partition.register_worker(slot),
            None => false,
        };
        if matched {
            debug!(
                queue = queue_name,
                partition = partition_id.0,
                worker = %worker_id,
                "Worker immediately matched with pending task"
            );
        } else {
            self.waiting_changed.notify_waiters();
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

    /// Free buffer slots across a queue's partitions (0 for unknown queues, which are
    /// created on first offer, so report the full capacity instead).
    pub fn free_capacity(&self, queue_name: &str) -> usize {
        let n = self.config.num_partitions;
        let mut known = false;
        let mut free = 0usize;
        for i in 0..n {
            if let Some(p) = self.partitions.get(&(queue_name.to_string(), i)) {
                known = true;
                free += p.max_buffer_size.saturating_sub(p.pending_tasks.len());
            }
        }
        if known {
            free
        } else {
            self.config.max_buffer_per_partition * n as usize
        }
    }

    /// Total buffered (not yet dispatched) tasks for a queue.
    pub fn buffered(&self, queue_name: &str) -> usize {
        (0..self.config.num_partitions)
            .filter_map(|i| {
                self.partitions
                    .get(&(queue_name.to_string(), i))
                    .map(|p| p.pending_tasks.len())
            })
            .sum()
    }
}
