use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub id: String,
    pub queue_name: String,
    pub task_name: String,
    pub status: String,
    pub priority: i32,
    pub max_retries: i32,
    pub attempt_count: i32,
    pub timeout_seconds: i32,
    pub idempotency_key: Option<String>,
    pub input: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub scheduled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskRun {
    pub id: String,
    pub task_id: String,
    pub attempt_number: i32,
    pub status: String,
    pub worker_id: Option<String>,
    pub assigned_node_id: Option<String>,
    pub output: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub lease_expires_at: String,
    pub last_heartbeat: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskLog {
    pub id: i64,
    pub task_run_id: String,
    pub level: String,
    pub message: String,
    pub timestamp_ms: i64,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Worker {
    pub id: String,
    pub name: String,
    pub queues: Vec<String>,
    pub concurrency: i32,
    pub active_tasks: i32,
    pub status: String,
    pub last_heartbeat: String,
    pub connected_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeadLetter {
    pub id: i64,
    pub task_id: String,
    pub queue_name: String,
    pub task_name: String,
    pub error_message: Option<String>,
    pub created_at: String,
    pub attempt_count: i32,
    pub input: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskSignal {
    pub id: String,
    pub task_id: String,
    pub signal_name: String,
    pub payload: Option<serde_json::Value>,
    pub status: String,
    pub created_at: String,
    pub delivered_at: Option<String>,
    pub acknowledged_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub queue_name: String,
    pub task_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendSignalRequest {
    pub signal_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendSignalResponse {
    pub signal_id: String,
    pub delivered: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RawTaskEvent {
    pub event_id: String,
    pub task_id: String,
    pub queue_name: String,
    pub new_status: i32,
    pub timestamp_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskEvent {
    pub event_id: String,
    pub task_id: String,
    pub queue_name: String,
    pub status: String,
    pub timestamp: String,
}
