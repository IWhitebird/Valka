//! Read models handed to the API layers. JSON shapes match the pre-WAL REST responses.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use valka_core::TaskStatus;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TaskView {
    pub id: String,
    pub queue_name: String,
    pub task_name: String,
    pub status: TaskStatus,
    pub priority: i32,
    pub max_retries: i32,
    pub attempt_count: i32,
    pub timeout_seconds: i32,
    pub idempotency_key: Option<String>,
    pub input: Option<Value>,
    pub metadata: Value,
    pub output: Option<Value>,
    pub error_message: Option<String>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TaskView {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "queue_name": self.queue_name,
            "task_name": self.task_name,
            "status": self.status.as_str(),
            "priority": self.priority,
            "max_retries": self.max_retries,
            "attempt_count": self.attempt_count,
            "timeout_seconds": self.timeout_seconds,
            "idempotency_key": self.idempotency_key,
            "input": self.input,
            "metadata": self.metadata,
            "output": self.output,
            "error_message": self.error_message,
            "scheduled_at": self.scheduled_at.map(|t| t.to_rfc3339()),
            "created_at": self.created_at.to_rfc3339(),
            "updated_at": self.updated_at.to_rfc3339(),
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunView {
    pub id: String,
    pub task_id: String,
    pub attempt_number: i32,
    pub worker_id: String,
    pub assigned_node_id: String,
    pub status: String,
    pub output: Option<Value>,
    pub error_message: Option<String>,
    pub lease_expires_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_heartbeat: DateTime<Utc>,
}

impl RunView {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "task_id": self.task_id,
            "attempt_number": self.attempt_number,
            "worker_id": self.worker_id,
            "assigned_node_id": self.assigned_node_id,
            "status": self.status,
            "output": self.output,
            "error_message": self.error_message,
            "lease_expires_at": self.lease_expires_at.to_rfc3339(),
            "started_at": self.started_at.to_rfc3339(),
            "completed_at": self.completed_at.map(|t| t.to_rfc3339()),
            "last_heartbeat": self.last_heartbeat.to_rfc3339(),
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SignalView {
    pub id: String,
    pub task_id: String,
    pub signal_name: String,
    pub payload: Option<Value>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub acknowledged_at: Option<DateTime<Utc>>,
}

impl SignalView {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "task_id": self.task_id,
            "signal_name": self.signal_name,
            "payload": self.payload,
            "status": self.status,
            "created_at": self.created_at.to_rfc3339(),
            "delivered_at": self.delivered_at.map(|t| t.to_rfc3339()),
            "acknowledged_at": self.acknowledged_at.map(|t| t.to_rfc3339()),
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CheckpointView {
    pub task_id: String,
    pub step: String,
    pub output: Value,
    pub run_id: String,
    pub attempt_number: i32,
    pub created_at: DateTime<Utc>,
}

impl CheckpointView {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "task_id": self.task_id,
            "step": self.step,
            "output": self.output,
            "run_id": self.run_id,
            "attempt_number": self.attempt_number,
            "created_at": self.created_at.to_rfc3339(),
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeadLetterView {
    pub id: String,
    pub task_id: String,
    pub queue_name: String,
    pub task_name: String,
    pub input: Option<Value>,
    pub error_message: Option<String>,
    pub attempt_count: i32,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

impl DeadLetterView {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "task_id": self.task_id,
            "queue_name": self.queue_name,
            "task_name": self.task_name,
            "input": self.input,
            "error_message": self.error_message,
            "attempt_count": self.attempt_count,
            "metadata": self.metadata,
            "created_at": self.created_at.to_rfc3339(),
        })
    }
}
