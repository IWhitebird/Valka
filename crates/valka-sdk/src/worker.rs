use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tracing::{error, info, warn};
use uuid::Uuid;

use valka_proto::worker_service_client::WorkerServiceClient;
use valka_proto::*;
use valka_proto::{worker_request, worker_response};

use crate::context::TaskContext;
use crate::error::SdkError;
use crate::retry::RetryPolicy;

pub type TaskHandler = Arc<
    dyn Fn(TaskContext) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send>>
        + Send
        + Sync,
>;

/// Builder for creating a ValkaWorker.
pub struct ValkaWorkerBuilder {
    name: String,
    server_addr: String,
    queues: Vec<String>,
    concurrency: i32,
    handler: Option<TaskHandler>,
    metadata: String,
}

impl ValkaWorkerBuilder {
    pub fn new() -> Self {
        Self {
            name: format!("worker-{}", &Uuid::now_v7().to_string()[..8]),
            server_addr: "http://[::1]:50051".to_string(),
            queues: vec![],
            concurrency: 1,
            handler: None,
            metadata: String::new(),
        }
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn server_addr(mut self, addr: &str) -> Self {
        self.server_addr = addr.to_string();
        self
    }

    pub fn queues(mut self, queues: &[&str]) -> Self {
        self.queues = queues.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn concurrency(mut self, n: i32) -> Self {
        self.concurrency = n;
        self
    }

    pub fn handler<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(TaskContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, String>> + Send + 'static,
    {
        self.handler = Some(Arc::new(move |ctx| Box::pin(f(ctx))));
        self
    }

    pub fn metadata(mut self, metadata: &str) -> Self {
        self.metadata = metadata.to_string();
        self
    }

    pub async fn build(self) -> Result<ValkaWorker, SdkError> {
        let handler = self
            .handler
            .ok_or_else(|| SdkError::Handler("No handler provided".to_string()))?;

        Ok(ValkaWorker {
            worker_id: Uuid::now_v7().to_string(),
            name: self.name,
            server_addr: self.server_addr,
            queues: self.queues,
            concurrency: self.concurrency,
            handler,
            metadata: self.metadata,
        })
    }
}

impl Default for ValkaWorkerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A Valka worker that connects to the control plane and processes tasks.
pub struct ValkaWorker {
    worker_id: String,
    name: String,
    server_addr: String,
    queues: Vec<String>,
    concurrency: i32,
    handler: TaskHandler,
    metadata: String,
}

impl ValkaWorker {
    pub fn builder() -> ValkaWorkerBuilder {
        ValkaWorkerBuilder::new()
    }

    /// Run the worker event loop. Blocks until shutdown.
    pub async fn run(self) -> Result<(), SdkError> {
        let mut retry_policy = RetryPolicy::new();

        loop {
            match self.connect_and_run(&mut retry_policy).await {
                Ok(()) => {
                    info!("Worker disconnected gracefully");
                    return Ok(());
                }
                Err(e) => {
                    let delay = retry_policy.next_delay();
                    warn!(
                        error = %e,
                        retry_in_ms = delay.as_millis(),
                        "Worker connection lost, reconnecting..."
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn connect_and_run(&self, retry_policy: &mut RetryPolicy) -> Result<(), SdkError> {
        let channel = Channel::from_shared(self.server_addr.clone())
            .map_err(|e| SdkError::Connection(e.to_string()))?
            .connect()
            .await?;

        let mut client = WorkerServiceClient::new(channel);

        // Set up bidirectional stream
        let (request_tx, request_rx) = mpsc::channel::<WorkerRequest>(256);
        let outbound = ReceiverStream::new(request_rx);

        let response = client.session(outbound).await?;
        let mut inbound = response.into_inner();

        retry_policy.reset();
        info!(worker_id = %self.worker_id, name = %self.name, "Connected to server");

        // Send hello
        let hello = WorkerRequest {
            request: Some(worker_request::Request::Hello(WorkerHello {
                worker_id: self.worker_id.clone(),
                worker_name: self.name.clone(),
                queues: self.queues.clone(),
                concurrency: self.concurrency,
                metadata: self.metadata.clone(),
            })),
        };
        request_tx
            .send(hello)
            .await
            .map_err(|_| SdkError::NotConnected)?;

        // Start heartbeat loop
        let hb_tx = request_tx.clone();
        let _worker_id = self.worker_id.clone();
        let hb_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                let hb = WorkerRequest {
                    request: Some(worker_request::Request::Heartbeat(Heartbeat {
                        active_task_ids: vec![],
                        timestamp_ms: chrono::Utc::now().timestamp_millis(),
                    })),
                };
                if hb_tx.send(hb).await.is_err() {
                    break;
                }
            }
        });

        // Process incoming messages
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.concurrency as usize));

        loop {
            tokio::select! {
                msg = inbound.next() => {
                    match msg {
                        Some(Ok(response)) => {
                            match response.response {
                                Some(worker_response::Response::TaskAssignment(assignment)) => {
                                    let permit = semaphore.clone().acquire_owned().await
                                        .map_err(|_| SdkError::ShuttingDown)?;
                                    let handler = self.handler.clone();
                                    let tx = request_tx.clone();
                                    tokio::spawn(async move {
                                        let ctx = TaskContext::new(
                                            assignment.task_id.clone(),
                                            assignment.task_run_id.clone(),
                                            assignment.queue_name,
                                            assignment.task_name,
                                            assignment.attempt_number,
                                            assignment.input,
                                            assignment.metadata,
                                            tx.clone(),
                                        );

                                        let result = handler(ctx).await;

                                        let task_result = match result {
                                            Ok(output) => TaskResult {
                                                task_id: assignment.task_id,
                                                task_run_id: assignment.task_run_id,
                                                success: true,
                                                retryable: false,
                                                output: output.to_string(),
                                                error_message: String::new(),
                                            },
                                            Err(err) => TaskResult {
                                                task_id: assignment.task_id,
                                                task_run_id: assignment.task_run_id,
                                                success: false,
                                                retryable: true,
                                                output: String::new(),
                                                error_message: err,
                                            },
                                        };

                                        let request = WorkerRequest {
                                            request: Some(worker_request::Request::TaskResult(task_result)),
                                        };
                                        let _ = tx.send(request).await;
                                        drop(permit);
                                    });
                                }
                                Some(worker_response::Response::TaskCancellation(cancel)) => {
                                    info!(task_id = %cancel.task_id, "Task cancelled by server");
                                }
                                Some(worker_response::Response::HeartbeatAck(_)) => {}
                                Some(worker_response::Response::ServerShutdown(shutdown)) => {
                                    info!(reason = %shutdown.reason, "Server shutting down");
                                    break;
                                }
                                None => {}
                            }
                        }
                        Some(Err(e)) => {
                            error!(error = %e, "Stream error");
                            break;
                        }
                        None => {
                            info!("Server closed stream");
                            break;
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("SIGINT received, shutting down gracefully");
                    let shutdown = WorkerRequest {
                        request: Some(worker_request::Request::Shutdown(GracefulShutdown {
                            reason: "SIGINT".to_string(),
                        })),
                    };
                    let _ = request_tx.send(shutdown).await;
                    // Wait for in-flight tasks
                    let _ = semaphore.acquire_many(self.concurrency as u32).await;
                    hb_handle.abort();
                    return Ok(());
                }
            }
        }

        hb_handle.abort();
        Err(SdkError::Connection("Stream closed".to_string()))
    }
}
