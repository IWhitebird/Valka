use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::future::Future;
use std::sync::Mutex;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tonic::transport::Channel;
use valka_proto::worker_service_client::WorkerServiceClient;
use valka_proto::{
    CheckpointRequest, LogEntry, SignalAck, TaskAssignment, TaskSignal, WorkerRequest,
    worker_request,
};

use crate::error::SdkError;

/// Data from a received signal.
pub struct SignalData {
    pub signal_id: String,
    pub name: String,
    pub payload: String,
}

impl SignalData {
    /// Parse the signal payload JSON into a typed value.
    pub fn parse_payload<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(&self.payload)
    }
}

/// Context passed to task handlers, providing logging, metadata, signals and checkpoints.
pub struct TaskContext {
    pub task_id: String,
    pub task_run_id: String,
    pub queue_name: String,
    pub task_name: String,
    pub attempt_number: i32,
    pub input: String,
    pub metadata: String,
    request_tx: mpsc::Sender<WorkerRequest>,
    signal_rx: mpsc::Receiver<TaskSignal>,
    signal_buffer: VecDeque<TaskSignal>,
    worker: WorkerServiceClient<Channel>,
    checkpoints: Mutex<HashMap<String, String>>,
}

impl TaskContext {
    pub fn new(
        assignment: TaskAssignment,
        request_tx: mpsc::Sender<WorkerRequest>,
        signal_rx: mpsc::Receiver<TaskSignal>,
        worker: WorkerServiceClient<Channel>,
    ) -> Self {
        let checkpoints = assignment
            .checkpoints
            .into_iter()
            .map(|c| (c.step, c.output))
            .collect();
        Self {
            task_id: assignment.task_id,
            task_run_id: assignment.task_run_id,
            queue_name: assignment.queue_name,
            task_name: assignment.task_name,
            attempt_number: assignment.attempt_number,
            input: assignment.input,
            metadata: assignment.metadata,
            request_tx,
            signal_rx,
            signal_buffer: VecDeque::new(),
            worker,
            checkpoints: Mutex::new(checkpoints),
        }
    }

    /// Parse the input JSON
    pub fn input<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(&self.input)
    }

    /// Run `f` as the step `name`, or return its result from an earlier attempt that already
    /// checkpointed it. The result is checkpointed before this returns, so a retry of the
    /// task skips the step. A step that fails before its checkpoint lands runs again.
    pub async fn step<T, E, F, Fut>(&self, name: &str, f: F) -> Result<T, String>
    where
        T: Serialize + DeserializeOwned,
        E: Display,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        if let Some(done) = self.checkpoint_value(name).map_err(|e| e.to_string())? {
            return Ok(done);
        }
        let value = f().await.map_err(|e| e.to_string())?;
        self.checkpoint(name, &value)
            .await
            .map_err(|e| e.to_string())?;
        Ok(value)
    }

    /// Durably record `value` as the result of `step` for this task.
    pub async fn checkpoint<T: Serialize>(&self, step: &str, value: &T) -> Result<(), SdkError> {
        let output = serde_json::to_string(value)?;
        self.worker
            .clone()
            .checkpoint(CheckpointRequest {
                task_id: self.task_id.clone(),
                task_run_id: self.task_run_id.clone(),
                step: step.to_string(),
                output: output.clone(),
            })
            .await?;
        self.checkpoints
            .lock()
            .expect("checkpoint map poisoned")
            .insert(step.to_string(), output);
        Ok(())
    }

    /// The checkpointed result of `step`, if any attempt of this task recorded one.
    pub fn checkpoint_value<T: DeserializeOwned>(&self, step: &str) -> Result<Option<T>, SdkError> {
        let checkpoints = self.checkpoints.lock().expect("checkpoint map poisoned");
        checkpoints
            .get(step)
            .map(|output| serde_json::from_str(output))
            .transpose()
            .map_err(SdkError::from)
    }

    /// Wait for a signal with a specific name. Non-matching signals are buffered.
    pub async fn wait_for_signal(&mut self, name: &str) -> Option<SignalData> {
        // Check buffer first
        if let Some(idx) = self
            .signal_buffer
            .iter()
            .position(|s| s.signal_name == name)
        {
            let signal = self.signal_buffer.remove(idx).unwrap();
            self.send_signal_ack(&signal.signal_id).await;
            return Some(SignalData {
                signal_id: signal.signal_id,
                name: signal.signal_name,
                payload: signal.payload,
            });
        }

        // Wait for matching signal from channel
        while let Some(signal) = self.signal_rx.recv().await {
            if signal.signal_name == name {
                self.send_signal_ack(&signal.signal_id).await;
                return Some(SignalData {
                    signal_id: signal.signal_id,
                    name: signal.signal_name,
                    payload: signal.payload,
                });
            }
            // Buffer non-matching signals
            self.signal_buffer.push_back(signal);
        }

        None
    }

    /// Receive the next signal (any name). Checks buffer first.
    pub async fn receive_signal(&mut self) -> Option<SignalData> {
        // Check buffer first
        if let Some(signal) = self.signal_buffer.pop_front() {
            self.send_signal_ack(&signal.signal_id).await;
            return Some(SignalData {
                signal_id: signal.signal_id,
                name: signal.signal_name,
                payload: signal.payload,
            });
        }

        // Wait for next signal from channel
        if let Some(signal) = self.signal_rx.recv().await {
            self.send_signal_ack(&signal.signal_id).await;
            return Some(SignalData {
                signal_id: signal.signal_id,
                name: signal.signal_name,
                payload: signal.payload,
            });
        }

        None
    }

    async fn send_signal_ack(&self, signal_id: &str) {
        let request = WorkerRequest {
            request: Some(worker_request::Request::SignalAck(SignalAck {
                signal_id: signal_id.to_string(),
            })),
        };
        let _ = self.request_tx.send(request).await;
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
