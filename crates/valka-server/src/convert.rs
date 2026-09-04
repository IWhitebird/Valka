//! Conversions between engine read models and proto / JSON.

use valka_core::TaskStatus;
use valka_engine::{EngineEvent, TaskView};
use valka_proto::{LogEntry, TaskEvent, TaskMeta};
use valka_wal::logstore::LogLine;

pub fn status_to_proto(s: TaskStatus) -> i32 {
    match s {
        TaskStatus::Pending => 1,
        TaskStatus::Dispatching => 2,
        TaskStatus::Running => 3,
        TaskStatus::Completed => 4,
        TaskStatus::Failed => 5,
        TaskStatus::Retry => 6,
        TaskStatus::DeadLetter => 7,
        TaskStatus::Cancelled => 8,
    }
}

pub fn proto_to_status(status: i32) -> Option<TaskStatus> {
    match status {
        1 => Some(TaskStatus::Pending),
        2 => Some(TaskStatus::Dispatching),
        3 => Some(TaskStatus::Running),
        4 => Some(TaskStatus::Completed),
        5 => Some(TaskStatus::Failed),
        6 => Some(TaskStatus::Retry),
        7 => Some(TaskStatus::DeadLetter),
        8 => Some(TaskStatus::Cancelled),
        _ => None,
    }
}

pub fn task_to_proto(t: TaskView) -> TaskMeta {
    TaskMeta {
        id: t.id,
        queue_name: t.queue_name,
        task_name: t.task_name,
        status: status_to_proto(t.status),
        priority: t.priority,
        max_retries: t.max_retries,
        attempt_count: t.attempt_count,
        timeout_seconds: t.timeout_seconds,
        idempotency_key: t.idempotency_key.unwrap_or_default(),
        input: t.input.map(|v| v.to_string()).unwrap_or_default(),
        metadata: t.metadata.to_string(),
        output: t.output.map(|v| v.to_string()).unwrap_or_default(),
        error_message: t.error_message.unwrap_or_default(),
        scheduled_at: t.scheduled_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
        created_at: t.created_at.to_rfc3339(),
        updated_at: t.updated_at.to_rfc3339(),
    }
}

pub fn event_to_proto(e: EngineEvent) -> TaskEvent {
    TaskEvent {
        event_id: uuid::Uuid::now_v7().to_string(),
        task_id: e.task_id,
        queue_name: e.queue_name,
        previous_status: e.previous.map(status_to_proto).unwrap_or(0),
        new_status: e.new.map(status_to_proto).unwrap_or(0),
        worker_id: e.worker_id.unwrap_or_default(),
        node_id: e.node_id,
        attempt_number: e.attempt,
        error_message: e.error.unwrap_or_default(),
        timestamp_ms: e.ts.timestamp_millis(),
    }
}

pub fn log_line_to_proto(l: LogLine) -> LogEntry {
    LogEntry {
        task_run_id: l.task_run_id,
        timestamp_ms: l.timestamp_ms,
        level: str_to_log_level(&l.level),
        message: l.message,
        metadata: l.metadata.map(|m| m.to_string()).unwrap_or_default(),
    }
}

pub fn log_line_to_json(l: &LogLine, idx: usize) -> serde_json::Value {
    serde_json::json!({
        "id": idx,
        "task_run_id": l.task_run_id,
        "timestamp_ms": l.timestamp_ms,
        "level": l.level,
        "message": l.message,
        "metadata": l.metadata,
    })
}

pub fn str_to_log_level(s: &str) -> i32 {
    match s {
        "DEBUG" => 1,
        "INFO" => 2,
        "WARN" => 3,
        "ERROR" => 4,
        _ => 0,
    }
}

/// Fan engine events out to the proto broadcast channel used by SSE / gRPC subscribers.
pub fn spawn_event_bridge(
    engine: &valka_engine::Engine,
    event_tx: tokio::sync::broadcast::Sender<TaskEvent>,
) {
    let mut rx = engine.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let _ = event_tx.send(event_to_proto(ev));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "event bridge lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
