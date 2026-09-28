use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{Mutex, Notify, Semaphore, mpsc};
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
            server_addr: "http://127.0.0.1:50051".to_string(),
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
            shutdown: Arc::new(Notify::new()),
            slots: Arc::new(Semaphore::new(self.concurrency.max(1) as usize)),
            session: Arc::new(Session::default()),
        })
    }
}

impl Default for ValkaWorkerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to request graceful shutdown of a running worker.
#[derive(Clone)]
pub struct ShutdownHandle(Arc<Notify>);

impl ShutdownHandle {
    /// Signal the worker to shut down gracefully, draining in-flight tasks.
    pub fn shutdown(&self) {
        self.0.notify_one();
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
    shutdown: Arc<Notify>,
    slots: Arc<Semaphore>,
    session: Arc<Session>,
}

/// State that outlives a single connection: tasks still running or awaiting a result ack
/// (heartbeated on whichever connection is current), results the server has not yet
/// acknowledged, and the current outbound channel.
#[derive(Default)]
struct Session {
    running: Mutex<HashSet<String>>,
    unacked: Mutex<HashMap<String, TaskResult>>,
    signals: Mutex<HashMap<String, mpsc::Sender<TaskSignal>>>,
    outbound: Mutex<Option<mpsc::Sender<WorkerRequest>>>,
}

const RESULT_RETRY_DELAY: Duration = Duration::from_secs(1);
const SHUTDOWN_ACK_WAIT: Duration = Duration::from_secs(5);

impl Session {
    async fn send(&self, request: WorkerRequest) {
        let tx = self.outbound.lock().await.clone();
        if let Some(tx) = tx {
            let _ = tx.send(request).await;
        }
    }

    /// Keep the result until the server answers APPLIED or STALE; send it on the current
    /// connection. A connection that comes up later resends everything still unacked.
    async fn deliver(&self, result: TaskResult) {
        self.unacked
            .lock()
            .await
            .insert(result.task_run_id.clone(), result.clone());
        self.send(result_request(result)).await;
    }

    async fn on_ack(self: &Arc<Self>, ack: ResultAck) {
        match ResultStatus::try_from(ack.status).unwrap_or(ResultStatus::Unspecified) {
            ResultStatus::Applied | ResultStatus::Stale => {
                if ack.status == ResultStatus::Stale as i32 {
                    warn!(
                        task_id = %ack.task_id,
                        "result not recorded: the run already ended another way"
                    );
                }
                self.unacked.lock().await.remove(&ack.task_run_id);
                self.running.lock().await.remove(&ack.task_id);
            }
            ResultStatus::Retry | ResultStatus::Unspecified => {
                let session = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(RESULT_RETRY_DELAY).await;
                    let pending = session.unacked.lock().await.get(&ack.task_run_id).cloned();
                    if let Some(result) = pending {
                        session.send(result_request(result)).await;
                    }
                });
            }
        }
    }
}

fn result_request(result: TaskResult) -> WorkerRequest {
    WorkerRequest {
        request: Some(worker_request::Request::TaskResult(result)),
    }
}

impl ValkaWorker {
    pub fn builder() -> ValkaWorkerBuilder {
        ValkaWorkerBuilder::new()
    }

    /// Returns a handle that can be used to trigger graceful shutdown from another task.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle(self.shutdown.clone())
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
            .http2_keep_alive_interval(Duration::from_secs(10))
            .keep_alive_timeout(Duration::from_secs(5))
            .keep_alive_while_idle(true)
            .connect()
            .await?;

        let mut client = WorkerServiceClient::new(channel);
        let (request_tx, request_rx) = mpsc::channel::<WorkerRequest>(256);
        let response = client.session(ReceiverStream::new(request_rx)).await?;
        let mut inbound = response.into_inner();

        retry_policy.reset();
        info!(worker_id = %self.worker_id, name = %self.name, "Connected to server");

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

        // Register this connection before collecting unacked results: a result delivered
        // concurrently is then either in the snapshot below or sent on this connection.
        *self.session.outbound.lock().await = Some(request_tx.clone());
        let unacked: Vec<TaskResult> = self
            .session
            .unacked
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        for result in unacked {
            let _ = request_tx.send(result_request(result)).await;
        }

        let hb_tx = request_tx.clone();
        let hb_session = self.session.clone();
        let hb_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let task_ids: Vec<String> =
                    hb_session.running.lock().await.iter().cloned().collect();
                let hb = WorkerRequest {
                    request: Some(worker_request::Request::Heartbeat(Heartbeat {
                        active_task_ids: task_ids,
                        timestamp_ms: chrono::Utc::now().timestamp_millis(),
                    })),
                };
                if hb_tx.send(hb).await.is_err() {
                    break;
                }
            }
        });

        let mut draining: Option<Pin<Box<dyn Future<Output = ()> + Send>>> = None;
        let outcome = loop {
            tokio::select! {
                msg = inbound.next() => {
                    match msg {
                        Some(Ok(response)) => match response.response {
                            Some(worker_response::Response::TaskAssignment(assignment)) => {
                                self.start_task(assignment, client.clone()).await;
                            }
                            Some(worker_response::Response::TaskCancellation(cancel)) => {
                                info!(task_id = %cancel.task_id, "Task cancelled by server");
                                self.session.running.lock().await.remove(&cancel.task_id);
                                self.session.signals.lock().await.remove(&cancel.task_id);
                            }
                            Some(worker_response::Response::TaskSignal(signal)) => {
                                let tx = self.session.signals.lock().await.get(&signal.task_id).cloned();
                                if let Some(tx) = tx
                                    && tx.send(signal).await.is_err()
                                {
                                    warn!("Signal channel closed for task");
                                }
                            }
                            Some(worker_response::Response::ResultAck(ack)) => {
                                self.session.on_ack(ack).await;
                            }
                            Some(worker_response::Response::HeartbeatAck(_)) => {}
                            Some(worker_response::Response::ServerShutdown(shutdown)) => {
                                info!(reason = %shutdown.reason, "Server shutting down");
                                break Err(SdkError::Connection("server shutting down".into()));
                            }
                            None => {}
                        },
                        Some(Err(_)) | None if draining.is_some() => break Ok(()),
                        Some(Err(e)) => {
                            error!(error = %e, "Stream error");
                            break Err(SdkError::Connection("stream error".into()));
                        }
                        None => {
                            info!("Server closed stream");
                            break Err(SdkError::Connection("Stream closed".to_string()));
                        }
                    }
                }
                _ = async { draining.as_mut().expect("guarded").await }, if draining.is_some() => {
                    break Ok(());
                }
                _ = tokio::signal::ctrl_c(), if draining.is_none() => {
                    info!("SIGINT received, shutting down gracefully");
                    draining = Some(Box::pin(self.drain(request_tx.clone(), "SIGINT")));
                }
                _ = self.shutdown.notified(), if draining.is_none() => {
                    info!("Shutdown requested via handle, draining gracefully");
                    draining = Some(Box::pin(self.drain(request_tx.clone(), "shutdown_handle")));
                }
            }
        };

        hb_handle.abort();
        *self.session.outbound.lock().await = None;
        outcome
    }

    /// Run the handler without ever blocking the receive loop: the slot is taken inside
    /// the spawned task, so cancels, signals and acks keep flowing at full capacity.
    async fn start_task(&self, assignment: TaskAssignment, rpc: WorkerServiceClient<Channel>) {
        let task_id = assignment.task_id.clone();
        self.session.running.lock().await.insert(task_id.clone());
        let (sig_tx, sig_rx) = mpsc::channel::<TaskSignal>(64);
        self.session
            .signals
            .lock()
            .await
            .insert(task_id.clone(), sig_tx);

        // Logs and signal acks follow whichever connection is current, so a task that
        // outlives a reconnect keeps reporting.
        let (request_tx, mut requests) = mpsc::channel::<WorkerRequest>(256);
        let forward = self.session.clone();
        tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                forward.send(request).await;
            }
        });

        let slots = self.slots.clone();
        let handler = self.handler.clone();
        let session = self.session.clone();
        tokio::spawn(async move {
            let Ok(permit) = slots.acquire_owned().await else {
                return;
            };
            let task_run_id = assignment.task_run_id.clone();
            let ctx = TaskContext::new(assignment, request_tx, sig_rx, rpc);
            let result = match handler(ctx).await {
                Ok(output) => TaskResult {
                    task_id: task_id.clone(),
                    task_run_id,
                    success: true,
                    retryable: false,
                    output: output.to_string(),
                    error_message: String::new(),
                },
                Err(err) => TaskResult {
                    task_id: task_id.clone(),
                    task_run_id,
                    success: false,
                    retryable: true,
                    output: String::new(),
                    error_message: err,
                },
            };
            session.signals.lock().await.remove(&task_id);
            drop(permit);
            session.deliver(result).await;
        });
    }

    /// Stop receiving tasks, wait for in-flight handlers, then give their results a moment
    /// to be acknowledged. The receive loop keeps running meanwhile, so acks arrive.
    fn drain(
        &self,
        request_tx: mpsc::Sender<WorkerRequest>,
        reason: &str,
    ) -> impl Future<Output = ()> + Send + 'static {
        let shutdown = WorkerRequest {
            request: Some(worker_request::Request::Shutdown(GracefulShutdown {
                reason: reason.to_string(),
            })),
        };
        let slots = self.slots.clone();
        let all_slots = self.concurrency.max(1) as u32;
        let session = self.session.clone();
        async move {
            let _ = request_tx.send(shutdown).await;
            let _ = slots.acquire_many(all_slots).await;
            let deadline = tokio::time::Instant::now() + SHUTDOWN_ACK_WAIT;
            while !session.unacked.lock().await.is_empty() && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}
