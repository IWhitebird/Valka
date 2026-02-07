use std::collections::VecDeque;
use tokio::sync::oneshot;
use valka_core::{PartitionId, WorkerId};

/// A task envelope passed through the matching service
#[derive(Debug)]
pub struct TaskEnvelope {
    pub task_id: String,
    pub task_run_id: String,
    pub queue_name: String,
    pub task_name: String,
    pub input: Option<String>,
    pub attempt_number: i32,
    pub timeout_seconds: i32,
    pub metadata: String,
    pub priority: i32,
}

/// A worker slot waiting for a task assignment
pub struct WorkerSlot {
    pub worker_id: WorkerId,
    pub task_sender: oneshot::Sender<TaskEnvelope>,
}

impl std::fmt::Debug for WorkerSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerSlot")
            .field("worker_id", &self.worker_id)
            .finish()
    }
}

/// A partition queue holding waiting workers and pending tasks
pub struct PartitionQueue {
    pub partition_id: PartitionId,
    pub queue_name: String,
    pub waiting_workers: VecDeque<WorkerSlot>,
    pub pending_tasks: VecDeque<TaskEnvelope>,
    pub parent: Option<PartitionId>,
    pub children: Vec<PartitionId>,
    pub max_buffer_size: usize,
}

impl PartitionQueue {
    pub fn new(
        partition_id: PartitionId,
        queue_name: String,
        parent: Option<PartitionId>,
        max_buffer_size: usize,
    ) -> Self {
        Self {
            partition_id,
            queue_name,
            waiting_workers: VecDeque::new(),
            pending_tasks: VecDeque::new(),
            parent,
            children: Vec::new(),
            max_buffer_size,
        }
    }

    /// Try to match a task with a waiting worker. Returns None if matched.
    pub fn try_match_task(&mut self, mut task: TaskEnvelope) -> Option<TaskEnvelope> {
        while let Some(slot) = self.waiting_workers.pop_front() {
            // Try to send; if receiver dropped, skip this worker
            match slot.task_sender.send(task) {
                Ok(()) => return None, // Matched!
                Err(returned_task) => {
                    // Worker disconnected, reclaim the task and try next
                    task = returned_task;
                    continue;
                }
            }
        }
        // No workers available, buffer the task
        Some(task)
    }

    /// Register a waiting worker. If there's a pending task, match immediately.
    pub fn register_worker(&mut self, slot: WorkerSlot) -> bool {
        if let Some(task) = self.pending_tasks.pop_front() {
            match slot.task_sender.send(task) {
                Ok(()) => return true, // Matched immediately
                Err(task) => {
                    // Worker already gone, put task back
                    self.pending_tasks.push_front(task);
                    return false;
                }
            }
        }
        self.waiting_workers.push_back(slot);
        false
    }

    /// Buffer a task (when no workers available)
    pub fn buffer_task(&mut self, task: TaskEnvelope) -> bool {
        if self.pending_tasks.len() >= self.max_buffer_size {
            return false; // Buffer full
        }
        self.pending_tasks.push_back(task);
        true
    }
}
