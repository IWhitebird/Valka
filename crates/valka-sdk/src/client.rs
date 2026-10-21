use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use valka_proto::api_service_client::ApiServiceClient;
use valka_proto::*;

use crate::error::SdkError;
use crate::transport::{ApiKeyAuth, AuthChannel, api_key_from_env, connect_channel, is_transient};

/// Client for the Valka API (create, get, list, cancel tasks; send signals).
///
/// Give it every node's address (`ValkaClient::builder().addrs(..)`): any node takes any
/// request, so when a node goes away the client moves to the next one. Creates carry an
/// idempotency key (generated when you set none), so a create retried after a lost
/// response returns the task it already made instead of making a second one. Reads are
/// retried the same way; cancels and signals are not, because they are not idempotent.
#[derive(Clone)]
pub struct ValkaClient {
    nodes: Arc<Vec<String>>,
    auth: ApiKeyAuth,
    next: usize,
    conn: Option<ApiServiceClient<AuthChannel>>,
    attempts: u32,
    timeout: Duration,
    auto_idempotency: bool,
}

/// Options for [`ValkaClient`].
pub struct ValkaClientBuilder {
    nodes: Vec<String>,
    api_key: Option<String>,
    attempts: u32,
    timeout: Duration,
    auto_idempotency: bool,
}

impl ValkaClientBuilder {
    /// Add a node address (`http://host:50051`). Call it once per node.
    pub fn addr(mut self, addr: &str) -> Self {
        self.nodes.push(addr.to_string());
        self
    }

    /// Node addresses; any node takes any request.
    pub fn addrs<S: AsRef<str>>(mut self, addrs: &[S]) -> Self {
        self.nodes
            .extend(addrs.iter().map(|a| a.as_ref().to_string()));
        self
    }

    /// The API key sent as `authorization: Bearer <key>`. Defaults to `VALKA_API_KEY`.
    pub fn api_key(mut self, key: &str) -> Self {
        self.api_key = Some(key.to_string());
        self
    }

    /// Attempts per call for retryable failures (default 6; 1 disables retries).
    pub fn attempts(mut self, n: u32) -> Self {
        self.attempts = n.max(1);
        self
    }

    /// Time limit for one attempt of one call (default 10 s).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Generate an idempotency key for creates that have none (default on). Turned off,
    /// a create whose response was lost is not retried.
    pub fn auto_idempotency(mut self, on: bool) -> Self {
        self.auto_idempotency = on;
        self
    }

    /// Connect to the first reachable node.
    pub async fn connect(self) -> Result<ValkaClient, SdkError> {
        if self.nodes.is_empty() {
            return Err(SdkError::Connection("no node address given".into()));
        }
        let mut client = ValkaClient {
            nodes: Arc::new(self.nodes),
            auth: ApiKeyAuth::new(self.api_key.or_else(api_key_from_env).as_deref())?,
            next: 0,
            conn: None,
            attempts: self.attempts,
            timeout: self.timeout,
            auto_idempotency: self.auto_idempotency,
        };
        let mut last = None;
        for _ in 0..client.nodes.len() {
            match client.connection().await {
                Ok(_) => return Ok(client),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or(SdkError::NotConnected))
    }
}

impl ValkaClient {
    pub fn builder() -> ValkaClientBuilder {
        ValkaClientBuilder {
            nodes: Vec::new(),
            api_key: None,
            attempts: 6,
            timeout: Duration::from_secs(10),
            auto_idempotency: true,
        }
    }

    /// Connect to one node (the API key comes from `VALKA_API_KEY`, if set).
    pub async fn connect(addr: &str) -> Result<Self, SdkError> {
        Self::builder().addr(addr).connect().await
    }

    /// The connection to the current node, opening it if needed. A failure moves on to
    /// the next node.
    async fn connection(&mut self) -> Result<ApiServiceClient<AuthChannel>, SdkError> {
        if let Some(c) = &self.conn {
            return Ok(c.clone());
        }
        let addr = self.nodes[self.next % self.nodes.len()].clone();
        match connect_channel(&addr, self.timeout).await {
            Ok(channel) => {
                let c = ApiServiceClient::with_interceptor(channel, self.auth.clone());
                self.conn = Some(c.clone());
                Ok(c)
            }
            Err(e) => {
                self.next += 1;
                Err(e)
            }
        }
    }

    /// Drop the current connection and use the next node.
    fn rotate(&mut self) {
        self.conn = None;
        self.next += 1;
    }

    /// Run `call`, retrying retryable failures with backoff on the next node.
    async fn retrying<T, F, Fut>(&mut self, mut call: F) -> Result<T, SdkError>
    where
        F: FnMut(ApiServiceClient<AuthChannel>) -> Fut,
        Fut: Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    {
        let mut delay = Duration::from_millis(100);
        let mut last = SdkError::NotConnected;
        for attempt in 0..self.attempts {
            if attempt > 0 {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(3));
            }
            let client = match self.connection().await {
                Ok(c) => c,
                Err(e) => {
                    last = e;
                    continue;
                }
            };
            match tokio::time::timeout(self.timeout, call(client)).await {
                Ok(Ok(response)) => return Ok(response.into_inner()),
                Ok(Err(status)) if is_transient(&status) => {
                    last = status.into();
                    self.rotate();
                }
                Ok(Err(status)) => return Err(status.into()),
                Err(_) => {
                    last = tonic::Status::deadline_exceeded("no answer in time").into();
                    self.rotate();
                }
            }
        }
        Err(last)
    }

    /// Run `call` once on the current node (for calls that must not be repeated).
    async fn once<T, F, Fut>(&mut self, call: F) -> Result<T, SdkError>
    where
        F: FnOnce(ApiServiceClient<AuthChannel>) -> Fut,
        Fut: Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    {
        let client = self.connection().await?;
        match tokio::time::timeout(self.timeout, call(client)).await {
            Ok(Ok(response)) => Ok(response.into_inner()),
            Ok(Err(status)) => {
                if is_transient(&status) {
                    self.rotate();
                }
                Err(status.into())
            }
            Err(_) => {
                self.rotate();
                Err(tonic::Status::deadline_exceeded("no answer in time").into())
            }
        }
    }

    pub async fn create_task(
        &mut self,
        queue_name: &str,
        task_name: &str,
        input: Option<serde_json::Value>,
    ) -> Result<TaskMeta, SdkError> {
        self.create_task_request(CreateTaskRequest {
            queue_name: queue_name.to_string(),
            task_name: task_name.to_string(),
            input: input.map(|v| v.to_string()).unwrap_or_default(),
            priority: 0,
            max_retries: 3,
            timeout_seconds: 300,
            idempotency_key: String::new(),
            metadata: String::new(),
            scheduled_at: String::new(),
        })
        .await
    }

    /// Create a task with every option of the API (priority, retries, timeout,
    /// idempotency key, metadata, schedule). A second create with the same idempotency key
    /// returns the first task.
    pub async fn create_task_request(
        &mut self,
        mut request: CreateTaskRequest,
    ) -> Result<TaskMeta, SdkError> {
        let retry_safe = !request.idempotency_key.is_empty() || self.auto_idempotency;
        if request.idempotency_key.is_empty() && self.auto_idempotency {
            request.idempotency_key = format!("auto-{}", uuid::Uuid::now_v7());
        }
        let response = if retry_safe {
            self.retrying(|mut c| {
                let r = request.clone();
                async move { c.create_task(r).await }
            })
            .await?
        } else {
            self.once(|mut c| async move { c.create_task(request).await })
                .await?
        };
        response
            .task
            .ok_or_else(|| SdkError::Handler("No task in response".to_string()))
    }

    pub async fn get_task(&mut self, task_id: &str) -> Result<TaskMeta, SdkError> {
        let req = GetTaskRequest {
            task_id: task_id.to_string(),
        };
        self.retrying(|mut c| {
            let r = req.clone();
            async move { c.get_task(r).await }
        })
        .await?
        .task
        .ok_or_else(|| SdkError::Handler("No task in response".to_string()))
    }

    pub async fn list_tasks(
        &mut self,
        queue_name: Option<&str>,
        status: Option<i32>,
        page_size: i32,
    ) -> Result<Vec<TaskMeta>, SdkError> {
        let req = ListTasksRequest {
            queue_name: queue_name.unwrap_or_default().to_string(),
            status: status.unwrap_or(0),
            pagination: Some(Pagination {
                page_size,
                page_token: String::new(),
            }),
        };
        Ok(self
            .retrying(|mut c| {
                let r = req.clone();
                async move { c.list_tasks(r).await }
            })
            .await?
            .tasks)
    }

    pub async fn cancel_task(&mut self, task_id: &str) -> Result<TaskMeta, SdkError> {
        let req = CancelTaskRequest {
            task_id: task_id.to_string(),
        };
        self.once(|mut c| async move { c.cancel_task(req).await })
            .await?
            .task
            .ok_or_else(|| SdkError::Handler("No task in response".to_string()))
    }

    pub async fn send_signal(
        &mut self,
        task_id: &str,
        signal_name: &str,
        payload: Option<serde_json::Value>,
    ) -> Result<(String, bool), SdkError> {
        let req = SendSignalRequest {
            task_id: task_id.to_string(),
            signal_name: signal_name.to_string(),
            payload: payload.map(|v| v.to_string()).unwrap_or_default(),
        };
        let resp = self
            .once(|mut c| async move { c.send_signal(req).await })
            .await?;
        Ok((resp.signal_id, resp.delivered))
    }
}
