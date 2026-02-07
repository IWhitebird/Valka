use chrono::{DateTime, Utc};
use std::collections::HashSet;
use tokio::sync::mpsc;
use valka_core::WorkerId;
use valka_proto::WorkerResponse;

/// Represents a connected worker and its communication channel.
pub struct WorkerHandle {
    pub worker_id: WorkerId,
    pub worker_name: String,
    pub queues: Vec<String>,
    pub concurrency: i32,
    pub active_tasks: HashSet<String>,
    pub response_tx: mpsc::Sender<WorkerResponse>,
    pub last_heartbeat: DateTime<Utc>,
    pub connected_at: DateTime<Utc>,
    pub metadata: String,
}

impl WorkerHandle {
    pub fn new(
        worker_id: WorkerId,
        worker_name: String,
        queues: Vec<String>,
        concurrency: i32,
        response_tx: mpsc::Sender<WorkerResponse>,
        metadata: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            worker_id,
            worker_name,
            queues,
            concurrency,
            active_tasks: HashSet::new(),
            response_tx,
            last_heartbeat: now,
            connected_at: now,
            metadata,
        }
    }

    pub fn available_slots(&self) -> i32 {
        self.concurrency - self.active_tasks.len() as i32
    }

    pub fn is_idle(&self) -> bool {
        self.active_tasks.is_empty()
    }

    pub fn assign_task(&mut self, task_id: String) {
        self.active_tasks.insert(task_id);
    }

    pub fn complete_task(&mut self, task_id: &str) {
        self.active_tasks.remove(task_id);
    }

    pub fn update_heartbeat(&mut self) {
        self.last_heartbeat = Utc::now();
    }
}
