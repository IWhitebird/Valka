use tokio::sync::mpsc;
use valka_proto::{LogEntry, WorkerRequest, worker_request};

/// Context passed to task handlers, providing logging and metadata access.
#[derive(Clone)]
pub struct TaskContext {
    pub task_id: String,
    pub task_run_id: String,
    pub queue_name: String,
    pub task_name: String,
    pub attempt_number: i32,
    pub input: String,
    pub metadata: String,
    request_tx: mpsc::Sender<WorkerRequest>,
}

impl TaskContext {
    pub fn new(
        task_id: String,
        task_run_id: String,
        queue_name: String,
        task_name: String,
        attempt_number: i32,
        input: String,
        metadata: String,
        request_tx: mpsc::Sender<WorkerRequest>,
    ) -> Self {
        Self {
            task_id,
            task_run_id,
            queue_name,
            task_name,
            attempt_number,
            input,
            metadata,
            request_tx,
        }
    }

    /// Parse the input JSON
    pub fn input<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(&self.input)
    }

    /// Log a message at INFO level
    pub async fn log(&self, message: &str) {
        self.log_at_level(2, message).await;
    }

    /// Log a message at DEBUG level
    pub async fn debug(&self, message: &str) {
        self.log_at_level(1, message).await;
    }

    /// Log a message at WARN level
    pub async fn warn(&self, message: &str) {
        self.log_at_level(3, message).await;
    }

    /// Log a message at ERROR level
    pub async fn error(&self, message: &str) {
        self.log_at_level(4, message).await;
    }

    async fn log_at_level(&self, level: i32, message: &str) {
        let entry = LogEntry {
            task_run_id: self.task_run_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            level,
            message: message.to_string(),
            metadata: String::new(),
        };

        let batch = valka_proto::LogBatch {
            entries: vec![entry],
        };

        let request = WorkerRequest {
            request: Some(worker_request::Request::LogBatch(batch)),
        };

        let _ = self.request_tx.send(request).await;
    }
}
