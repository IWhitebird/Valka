use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use tokio::sync::{Mutex, Notify, Semaphore, mpsc};
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info, warn};
use uuid::Uuid;

use valka_proto::worker_service_client::WorkerServiceClient;
use valka_proto::*;
use valka_proto::{worker_request, worker_response};

use crate::context::TaskContext;
use crate::error::SdkError;
use crate::retry::RetryPolicy;
use crate::transport::{ApiKeyAuth, AuthChannel, api_key_from_env, connect_channel};

pub type TaskHandler = Arc<
    dyn Fn(TaskContext) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send>>
        + Send
        + Sync,
>;

/// Builder for creating a ValkaWorker.
pub struct ValkaWorkerBuilder {
    name: String,
    server_addrs: Vec<String>,
    api_key: Option<String>,
    queues: Vec<String>,
    concurrency: i32,
    handler: Option<TaskHandler>,
    metadata: String,
}

impl ValkaWorkerBuilder {
    pub fn new() -> Self {
        Self {
            name: format!("worker-{}", &Uuid::now_v7().to_string()[..8]),
            server_addrs: Vec::new(),
            api_key: None,
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

    /// The node to connect to (default `http://127.0.0.1:50051`).
    pub fn server_addr(mut self, addr: &str) -> Self {
        self.server_addrs = vec![addr.to_string()];
        self
    }

    /// Several nodes of one cluster: the worker connects to one and moves to the next when
    /// its connection drops. Its running tasks reattach on whichever node it reaches.
    pub fn server_addrs<S: AsRef<str>>(mut self, addrs: &[S]) -> Self {
        self.server_addrs = addrs.iter().map(|a| a.as_ref().to_string()).collect();
        self
    }

    /// The API key sent as `authorization: Bearer <key>`. Defaults to `VALKA_API_KEY`.
    pub fn api_key(mut self, key: &str) -> Self {
        self.api_key = Some(key.to_string());
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

        let server_addrs = if self.server_addrs.is_empty() {
            vec!["http://127.0.0.1:50051".to_string()]
        } else {
            self.server_addrs
        };
        Ok(ValkaWorker {
            worker_id: Uuid::now_v7().to_string(),
            name: self.name,
            server_addrs,
            next_addr: AtomicUsize::new(0),
            auth: ApiKeyAuth::new(self.api_key.or_else(api_key_from_env).as_deref())?,
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
    server_addrs: Vec<String>,
    next_addr: AtomicUsize,
    auth: ApiKeyAuth,
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
    /// task id -> the latest run of it this worker executes (sent in `WorkerHello.running`
    /// on every connect, so the server can reattach it to the new connection's node). An
    /// older run of the same task still finishing here is stale and never removes it.
    running: Mutex<HashMap<String, RunningTask>>,
    /// task run id -> its result, until the server answers APPLIED or STALE.
    unacked: Mutex<HashMap<String, TaskResult>>,
    /// task id -> (run id, the run's signal channel).
    signals: Mutex<HashMap<String, (String, mpsc::Sender<TaskSignal>)>>,
    outbound: Mutex<Option<mpsc::Sender<WorkerRequest>>>,
    /// Handlers running or waiting for a slot whose result is not yet in `unacked`.
    executing: AtomicUsize,
}

const RESULT_RETRY_DELAY: Duration = Duration::from_secs(1);
/// How long a drain waits for the results of finished handlers to be acknowledged.
const SHUTDOWN_ACK_WAIT: Duration = Duration::from_secs(10);
const DRAIN_POLL: Duration = Duration::from_millis(50);
/// Reconnect delay cap while draining: unacked results are waiting for a connection.
const DRAIN_RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// A graceful shutdown in progress. It outlives connections: a drain interrupted by a lost
/// connection or a `ServerShutdown` carries on over the next node.
struct Drain {
    reason: String,
    ack_wait: Duration,
    /// When the last handler finished; its result then has `ack_wait` to be acknowledged.
    idle_since: Option<Instant>,
}

impl Drain {
    fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
            ack_wait: SHUTDOWN_ACK_WAIT,
            idle_since: None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DrainState {
    /// Handlers are still running, or results still wait for their ack.
    Busy,
    /// Nothing runs and every result is acknowledged.
    Done,
    /// Results stayed unacknowledged for the whole ack wait.
    GaveUp,
}

/// Counts a handler as executing until it is dropped (also when the handler panics).
struct Executing(Arc<Session>);

impl Executing {
    fn new(session: Arc<Session>) -> Self {
        session.executing.fetch_add(1, Ordering::SeqCst);
        Self(session)
    }
}

impl Drop for Executing {
    fn drop(&mut self) {
        self.0.executing.fetch_sub(1, Ordering::SeqCst);
    }
}

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
                // Only this run's entry: a newer run of the same task may be running here.
                let mut running = self.running.lock().await;
                if running
                    .get(&ack.task_id)
                    .is_some_and(|r| r.task_run_id == ack.task_run_id)
                {
                    running.remove(&ack.task_id);
                }
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

    /// Where a drain stands: handlers first, then their results' acks (bounded).
    async fn drain_state(&self, drain: &mut Drain) -> DrainState {
        if self.executing.load(Ordering::SeqCst) > 0 {
            drain.idle_since = None;
            return DrainState::Busy;
        }
        if self.unacked.lock().await.is_empty() {
            return DrainState::Done;
        }
        let since = *drain.idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= drain.ack_wait {
            DrainState::GaveUp
        } else {
            DrainState::Busy
        }
    }
}

fn result_request(result: TaskResult) -> WorkerRequest {
    WorkerRequest {
        request: Some(worker_request::Request::TaskResult(result)),
    }
}

fn shutdown_request(reason: &str) -> WorkerRequest {
    WorkerRequest {
        request: Some(worker_request::Request::Shutdown(GracefulShutdown {
            reason: reason.to_string(),
        })),
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
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
    ///
    /// A shutdown (Ctrl-C or the [`ShutdownHandle`]) drains: the server stops sending tasks,
    /// running handlers finish and their results are acknowledged, then this returns. A
    /// connection lost meanwhile is re-established (on the next node) and the drain goes on.
    pub async fn run(self) -> Result<(), SdkError> {
        let notify = self.shutdown.clone();
        let ctrl_c = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("SIGINT received, shutting down gracefully");
                notify.notify_one();
            }
        });
        let outcome = self.run_sessions().await;
        ctrl_c.abort();
        outcome
    }

    async fn run_sessions(&self) -> Result<(), SdkError> {
        let mut retry_policy = RetryPolicy::new();
        let mut drain: Option<Drain> = None;

        loop {
            let e = match self.connect_and_run(&mut retry_policy, &mut drain).await {
                Ok(()) => {
                    info!("Worker drained and disconnected");
                    return Ok(());
                }
                Err(e) => e,
            };
            // Try the next node next time (any node takes any worker).
            self.next_addr.fetch_add(1, Ordering::Relaxed);
            let mut delay = retry_policy.next_delay();
            if drain.is_some() {
                delay = delay.min(DRAIN_RECONNECT_DELAY);
            }
            warn!(
                error = %e,
                retry_in_ms = delay.as_millis(),
                "Worker connection lost, reconnecting..."
            );
            if self.wait_to_reconnect(delay, &mut drain).await {
                info!("Worker drained while disconnected");
                return Ok(());
            }
        }
    }

    /// Sleep before the next connection attempt. A shutdown requested meanwhile starts a
    /// drain; returns true when the drain has nothing left to wait for.
    async fn wait_to_reconnect(&self, delay: Duration, drain: &mut Option<Drain>) -> bool {
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        let mut poll = tokio::time::interval(DRAIN_POLL);
        loop {
            tokio::select! {
                _ = &mut sleep => return false,
                _ = self.shutdown.notified(), if drain.is_none() => {
                    info!("Shutdown requested while disconnected, draining");
                    *drain = Some(Drain::new("shutdown"));
                    let soon = Instant::now() + DRAIN_RECONNECT_DELAY;
                    if sleep.deadline() > soon {
                        sleep.as_mut().reset(soon);
                    }
                }
                _ = poll.tick(), if drain.is_some() => {}
            }
            if let Some(d) = drain.as_mut() {
                match self.session.drain_state(d).await {
                    DrainState::Busy => {}
                    DrainState::Done => return true,
                    DrainState::GaveUp => {
                        warn!("results still unacknowledged after the drain's ack wait");
                        return true;
                    }
                }
            }
        }
    }

    /// One connection. `Ok` means a drain finished; any `Err` means reconnect.
    async fn connect_and_run(
        &self,
        retry_policy: &mut RetryPolicy,
        drain: &mut Option<Drain>,
    ) -> Result<(), SdkError> {
        let addr =
            &self.server_addrs[self.next_addr.load(Ordering::Relaxed) % self.server_addrs.len()];
        let channel = connect_channel(addr, Duration::from_secs(10)).await?;

        // The server bounds what a task carries (its payload limit); gRPC's 4 MiB default
        // would break the stream on a large assignment.
        let mut client = WorkerServiceClient::with_interceptor(channel, self.auth.clone())
            .max_decoding_message_size(usize::MAX);
        let (request_tx, request_rx) = mpsc::channel::<WorkerRequest>(256);
        let response = client.session(ReceiverStream::new(request_rx)).await?;
        let mut inbound = response.into_inner();

        info!(worker_id = %self.worker_id, name = %self.name, "Connected to server");

        let hello = WorkerRequest {
            request: Some(worker_request::Request::Hello(WorkerHello {
                worker_id: self.worker_id.clone(),
                worker_name: self.name.clone(),
                queues: self.queues.clone(),
                concurrency: self.concurrency,
                metadata: self.metadata.clone(),
                running: self
                    .session
                    .running
                    .lock()
                    .await
                    .values()
                    .cloned()
                    .collect(),
            })),
        };
        request_tx
            .send(hello)
            .await
            .map_err(|_| SdkError::NotConnected)?;
        // A drain carried over from an earlier connection: no new tasks on this one either.
        if let Some(d) = drain.as_ref() {
            request_tx
                .send(shutdown_request(&d.reason))
                .await
                .map_err(|_| SdkError::NotConnected)?;
        }

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
                    hb_session.running.lock().await.keys().cloned().collect();
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

        let mut poll = tokio::time::interval(DRAIN_POLL);
        let mut answered = false;
        let outcome = loop {
            tokio::select! {
                msg = inbound.next() => {
                    match msg {
                        Some(Ok(response)) => {
                            // The server took the session: back off from scratch next time.
                            if !answered {
                                answered = true;
                                retry_policy.reset();
                            }
                            match response.response {
                                Some(worker_response::Response::TaskAssignment(assignment)) => {
                                    self.start_task(assignment, client.clone()).await;
                                }
                                Some(worker_response::Response::TaskCancellation(cancel)) => {
                                    info!(task_id = %cancel.task_id, "Task cancelled by server");
                                    self.session.running.lock().await.remove(&cancel.task_id);
                                    self.session.signals.lock().await.remove(&cancel.task_id);
                                }
                                Some(worker_response::Response::TaskSignal(signal)) => {
                                    let tx = self
                                        .session
                                        .signals
                                        .lock()
                                        .await
                                        .get(&signal.task_id)
                                        .map(|(_, tx)| tx.clone());
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
                                    // Reconnect to another node; a drain in progress goes on there.
                                    info!(reason = %shutdown.reason, "Server shutting down");
                                    break Err(SdkError::Connection("server shutting down".into()));
                                }
                                None => {}
                            }
                        }
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
                _ = self.shutdown.notified(), if drain.is_none() => {
                    info!("Shutdown requested, draining gracefully");
                    let d = Drain::new("shutdown");
                    let _ = request_tx.send(shutdown_request(&d.reason)).await;
                    *drain = Some(d);
                }
                _ = poll.tick(), if drain.is_some() => {
                    if let Some(d) = drain.as_mut() {
                        match self.session.drain_state(d).await {
                            DrainState::Busy => {}
                            DrainState::Done => break Ok(()),
                            DrainState::GaveUp => {
                                warn!("results still unacknowledged after the drain's ack wait");
                                break Ok(());
                            }
                        }
                    }
                }
            }
        };

        hb_handle.abort();
        *self.session.outbound.lock().await = None;
        outcome
    }

    /// Run the handler without ever blocking the receive loop: the slot is taken inside
    /// the spawned task, so cancels, signals and acks keep flowing at full capacity.
    async fn start_task(&self, assignment: TaskAssignment, rpc: WorkerServiceClient<AuthChannel>) {
        let task_id = assignment.task_id.clone();
        let task_run_id = assignment.task_run_id.clone();
        // A newer run replaces an older one of the same task still finishing here: that
        // one is stale, and its result will be answered STALE.
        self.session.running.lock().await.insert(
            task_id.clone(),
            RunningTask {
                task_id: task_id.clone(),
                task_run_id: task_run_id.clone(),
                attempt_number: assignment.attempt_number,
            },
        );
        let (sig_tx, sig_rx) = mpsc::channel::<TaskSignal>(64);
        self.session
            .signals
            .lock()
            .await
            .insert(task_id.clone(), (task_run_id.clone(), sig_tx));

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
        // Counted from now on, so a drain waits for tasks that still wait for a slot.
        let executing = Executing::new(self.session.clone());
        tokio::spawn(async move {
            let _executing = executing;
            let Ok(permit) = slots.acquire_owned().await else {
                return;
            };
            let ctx = TaskContext::new(assignment, request_tx, sig_rx, rpc);
            let outcome = AssertUnwindSafe(async move { handler(ctx).await })
                .catch_unwind()
                .await;
            let (success, output, error_message) = match outcome {
                Ok(Ok(output)) => (true, output.to_string(), String::new()),
                Ok(Err(err)) => (false, String::new(), err),
                Err(panic) => {
                    let msg = panic_message(panic.as_ref());
                    error!(task_id = %task_id, panic = %msg, "Task handler panicked");
                    (false, String::new(), format!("handler panicked: {msg}"))
                }
            };
            let result = TaskResult {
                task_id: task_id.clone(),
                task_run_id: task_run_id.clone(),
                success,
                retryable: !success,
                output,
                error_message,
            };
            {
                let mut signals = session.signals.lock().await;
                if signals
                    .get(&task_id)
                    .is_some_and(|(run, _)| *run == task_run_id)
                {
                    signals.remove(&task_id);
                }
            }
            drop(permit);
            // `_executing` drops after this: a drain never sees the handler gone before its
            // result is in `unacked`.
            session.deliver(result).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio_stream::wrappers::TcpListenerStream;
    use valka_proto::worker_service_server::{WorkerService, WorkerServiceServer};

    fn running(task: &str, run: &str) -> RunningTask {
        RunningTask {
            task_id: task.into(),
            task_run_id: run.into(),
            attempt_number: 1,
        }
    }

    fn result(task: &str, run: &str) -> TaskResult {
        TaskResult {
            task_id: task.into(),
            task_run_id: run.into(),
            success: true,
            ..Default::default()
        }
    }

    fn ack(task: &str, run: &str, status: ResultStatus) -> ResultAck {
        ResultAck {
            task_id: task.into(),
            task_run_id: run.into(),
            status: status as i32,
            message: String::new(),
        }
    }

    #[tokio::test]
    async fn the_ack_of_an_older_run_leaves_the_newer_run_alone() {
        let s = Arc::new(Session::default());
        s.running
            .lock()
            .await
            .insert("t".into(), running("t", "r2"));
        s.deliver(result("t", "r1")).await;
        s.on_ack(ack("t", "r1", ResultStatus::Stale)).await;
        assert!(s.unacked.lock().await.is_empty());
        assert_eq!(s.running.lock().await["t"].task_run_id, "r2");

        s.deliver(result("t", "r2")).await;
        s.on_ack(ack("t", "r2", ResultStatus::Applied)).await;
        assert!(s.running.lock().await.is_empty());
        assert!(s.unacked.lock().await.is_empty());
    }

    #[tokio::test]
    async fn a_drain_waits_for_handlers_then_acks() {
        let s = Arc::new(Session::default());
        let mut d = Drain::new("test");
        d.ack_wait = Duration::from_millis(30);
        assert_eq!(s.drain_state(&mut d).await, DrainState::Done);

        let busy = Executing::new(s.clone());
        assert_eq!(s.drain_state(&mut d).await, DrainState::Busy);
        s.deliver(result("t", "r1")).await;
        drop(busy);
        assert_eq!(
            s.drain_state(&mut d).await,
            DrainState::Busy,
            "waiting for the ack"
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(s.drain_state(&mut d).await, DrainState::GaveUp);

        s.on_ack(ack("t", "r1", ResultStatus::Applied)).await;
        assert_eq!(s.drain_state(&mut d).await, DrainState::Done);
    }

    /// A node that records what a worker sends it and answers with a script.
    #[derive(Clone)]
    struct FakeNode {
        name: &'static str,
        log: mpsc::UnboundedSender<(&'static str, WorkerRequest)>,
        /// Sent right after the hello.
        on_hello: Vec<WorkerResponse>,
        /// Answer a GracefulShutdown with ServerShutdown and close the stream.
        leave_on_drain: bool,
    }

    #[tonic::async_trait]
    impl WorkerService for FakeNode {
        type SessionStream = ReceiverStream<Result<WorkerResponse, tonic::Status>>;

        async fn session(
            &self,
            request: tonic::Request<tonic::Streaming<WorkerRequest>>,
        ) -> Result<tonic::Response<Self::SessionStream>, tonic::Status> {
            let mut inbound = request.into_inner();
            let (tx, rx) = mpsc::channel(64);
            let node = self.clone();
            tokio::spawn(async move {
                while let Some(Ok(req)) = inbound.next().await {
                    let _ = node.log.send((node.name, req.clone()));
                    let reply = match req.request {
                        Some(worker_request::Request::Hello(_)) => node.on_hello.clone(),
                        Some(worker_request::Request::Shutdown(_)) if node.leave_on_drain => {
                            let bye = worker_response::Response::ServerShutdown(ServerShutdown {
                                reason: "restart".into(),
                                drain_seconds: 0,
                            });
                            let _ = tx
                                .send(Ok(WorkerResponse {
                                    response: Some(bye),
                                }))
                                .await;
                            return;
                        }
                        Some(worker_request::Request::TaskResult(r)) => {
                            vec![WorkerResponse {
                                response: Some(worker_response::Response::ResultAck(ack(
                                    &r.task_id,
                                    &r.task_run_id,
                                    ResultStatus::Applied,
                                ))),
                            }]
                        }
                        _ => vec![],
                    };
                    for r in reply {
                        let _ = tx.send(Ok(r)).await;
                    }
                }
            });
            Ok(tonic::Response::new(ReceiverStream::new(rx)))
        }

        async fn checkpoint(
            &self,
            _request: tonic::Request<CheckpointRequest>,
        ) -> Result<tonic::Response<CheckpointResponse>, tonic::Status> {
            Ok(tonic::Response::new(CheckpointResponse {}))
        }
    }

    async fn serve(node: FakeNode) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(WorkerServiceServer::new(node))
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );
        addr
    }

    /// A node restart in the middle of a drain: the worker moves to the next node, tells it
    /// it is draining, lists its running task, delivers the result there and only then stops.
    #[tokio::test]
    async fn a_drain_survives_a_server_shutdown() {
        let (log, mut seen) = mpsc::unbounded_channel();
        let assignment = WorkerResponse {
            response: Some(worker_response::Response::TaskAssignment(TaskAssignment {
                task_id: "t1".into(),
                task_run_id: "r1".into(),
                attempt_number: 1,
                ..Default::default()
            })),
        };
        let a = serve(FakeNode {
            name: "a",
            log: log.clone(),
            on_hello: vec![assignment],
            leave_on_drain: true,
        })
        .await;
        let b = serve(FakeNode {
            name: "b",
            log,
            on_hello: vec![],
            leave_on_drain: false,
        })
        .await;

        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (s, r) = (started.clone(), release.clone());
        let worker = ValkaWorker::builder()
            .server_addrs(&[format!("http://{a}"), format!("http://{b}")])
            .queues(&["q"])
            .handler(move |_ctx| {
                let (s, r) = (s.clone(), r.clone());
                async move {
                    s.notify_one();
                    r.notified().await;
                    Ok(serde_json::json!({"ok": true}))
                }
            })
            .build()
            .await
            .unwrap();
        let stop = worker.shutdown_handle();
        let run = tokio::spawn(worker.run_sessions_for_test());

        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("the task starts");
        stop.shutdown();

        // Node b hears the hello (listing the running task) and the drain.
        let mut on_b = Vec::new();
        while on_b.len() < 2 {
            let (node, req) = tokio::time::timeout(Duration::from_secs(5), seen.recv())
                .await
                .expect("the worker reaches node b")
                .unwrap();
            if node == "b" {
                on_b.push(req);
            }
        }
        let Some(worker_request::Request::Hello(hello)) = &on_b[0].request else {
            panic!("hello first, got {:?}", on_b[0]);
        };
        assert_eq!(hello.running.len(), 1);
        assert_eq!(hello.running[0].task_run_id, "r1");
        assert!(matches!(
            on_b[1].request,
            Some(worker_request::Request::Shutdown(_))
        ));
        assert!(
            !run.is_finished(),
            "the drain waits for the running handler"
        );

        release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("the worker stops once the result is acknowledged")
            .unwrap()
            .unwrap();
        let mut results = Vec::new();
        while let Ok((node, req)) = seen.try_recv() {
            if let Some(worker_request::Request::TaskResult(r)) = req.request {
                results.push((node, r.task_run_id));
            }
        }
        assert_eq!(results, vec![("b", "r1".to_string())]);
    }

    impl ValkaWorker {
        /// `run` without the process-wide Ctrl-C listener.
        async fn run_sessions_for_test(self) -> Result<(), SdkError> {
            self.run_sessions().await
        }
    }
}
