use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use futures::{Stream, StreamExt};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::info;

use crate::convert::{log_line_to_proto, proto_to_status, task_to_proto};
use crate::internal_grpc::InternalServiceImpl;
use valka_core::{NodeId, ServerError};
use valka_dispatcher::DispatcherService;
use valka_engine::{CreateTask, Engine, LogIngester};
use valka_proto::*;

pub struct ApiServiceImpl {
    engine: Engine,
    dispatcher: DispatcherService,
    event_tx: broadcast::Sender<TaskEvent>,
    logs: Arc<LogIngester>,
}

pub struct WorkerServiceImpl {
    dispatcher: DispatcherService,
    shutdown: watch::Receiver<bool>,
}

fn parse_json(field: &str, s: &str) -> Result<Option<serde_json::Value>, Status> {
    if s.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(s)
        .map(Some)
        .map_err(|e| Status::invalid_argument(format!("Invalid {field} JSON: {e}")))
}

#[tonic::async_trait]
impl api_service_server::ApiService for ApiServiceImpl {
    async fn create_task(
        &self,
        request: Request<CreateTaskRequest>,
    ) -> Result<Response<CreateTaskResponse>, Status> {
        let req = request.into_inner();
        let input = parse_json("input", &req.input)?;
        let metadata = parse_json("metadata", &req.metadata)?.unwrap_or(serde_json::json!({}));
        let scheduled_at = if req.scheduled_at.is_empty() {
            None
        } else {
            Some(
                req.scheduled_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .map_err(|e| Status::invalid_argument(format!("Invalid scheduled_at: {e}")))?,
            )
        };
        let task = self
            .engine
            .create_task(CreateTask {
                queue_name: req.queue_name,
                task_name: req.task_name,
                input,
                priority: req.priority,
                max_retries: if req.max_retries == 0 {
                    3
                } else {
                    req.max_retries
                },
                timeout_seconds: if req.timeout_seconds == 0 {
                    300
                } else {
                    req.timeout_seconds
                },
                idempotency_key: if req.idempotency_key.is_empty() {
                    None
                } else {
                    Some(req.idempotency_key)
                },
                metadata,
                scheduled_at,
            })
            .await
            .map_err(Status::from)?;
        Ok(Response::new(CreateTaskResponse {
            task: Some(task_to_proto(task)),
        }))
    }

    async fn get_task(
        &self,
        request: Request<GetTaskRequest>,
    ) -> Result<Response<GetTaskResponse>, Status> {
        let req = request.into_inner();
        let task = self
            .engine
            .get_task(&req.task_id)
            .ok_or_else(|| Status::not_found(format!("Task not found: {}", req.task_id)))?;
        Ok(Response::new(GetTaskResponse {
            task: Some(task_to_proto(task)),
        }))
    }

    async fn list_tasks(
        &self,
        request: Request<ListTasksRequest>,
    ) -> Result<Response<ListTasksResponse>, Status> {
        let req = request.into_inner();
        let queue_name = if req.queue_name.is_empty() {
            None
        } else {
            Some(req.queue_name.as_str())
        };
        let status_filter = if req.status == 0 {
            None
        } else {
            proto_to_status(req.status)
        };
        let (limit, offset) = if let Some(ref p) = req.pagination {
            let offset: usize = p.page_token.parse().unwrap_or(0);
            (p.page_size.max(1) as usize, offset)
        } else {
            (50, 0)
        };
        let tasks = self
            .engine
            .list_tasks(queue_name, status_filter, limit, offset);
        let next_token = if tasks.len() == limit {
            (offset + limit).to_string()
        } else {
            String::new()
        };
        Ok(Response::new(ListTasksResponse {
            tasks: tasks.into_iter().map(task_to_proto).collect(),
            next_page_token: next_token,
        }))
    }

    async fn cancel_task(
        &self,
        request: Request<CancelTaskRequest>,
    ) -> Result<Response<CancelTaskResponse>, Status> {
        let req = request.into_inner();
        let (task, running_on) = self
            .engine
            .cancel_task(&req.task_id, "Cancelled by user")
            .await
            .map_err(|e| match e {
                ServerError::TaskNotFound(_) | ServerError::InvalidStatusTransition { .. } => {
                    Status::failed_precondition(format!(
                        "Task {} not found or not in cancellable state",
                        req.task_id
                    ))
                }
                other => Status::from(other),
            })?;
        if running_on.is_some() {
            self.dispatcher.cancel_task_on_worker(&req.task_id).await;
        }
        Ok(Response::new(CancelTaskResponse {
            task: Some(task_to_proto(task)),
        }))
    }

    async fn send_signal(
        &self,
        request: Request<SendSignalRequest>,
    ) -> Result<Response<SendSignalResponse>, Status> {
        let req = request.into_inner();
        let payload = parse_json("payload", &req.payload)?;
        let signal = self
            .engine
            .send_signal(&req.task_id, &req.signal_name, payload)
            .await
            .map_err(|e| match e {
                ServerError::InvalidStatusTransition { from, .. } => Status::failed_precondition(
                    format!("Cannot send signal to task in {from} state"),
                ),
                other => Status::from(other),
            })?;
        let delivered = self
            .dispatcher
            .send_signal_to_worker(
                &req.task_id,
                TaskSignal {
                    signal_id: signal.id.clone(),
                    task_id: signal.task_id.clone(),
                    signal_name: signal.signal_name.clone(),
                    payload: signal.payload.map(|v| v.to_string()).unwrap_or_default(),
                    timestamp_ms: signal.created_at.timestamp_millis(),
                },
            )
            .await;
        if delivered {
            self.engine.signal_delivered(&signal.id);
        }
        Ok(Response::new(SendSignalResponse {
            signal_id: signal.id,
            delivered,
        }))
    }

    type SubscribeEventsStream =
        Pin<Box<dyn Stream<Item = Result<TaskEvent, Status>> + Send + 'static>>;

    async fn subscribe_events(
        &self,
        request: Request<SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        let filter = request.into_inner().queue_name;
        let mut rx = self.event_tx.subscribe();
        let (tx, rx_stream) = mpsc::channel(256);

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if !filter.is_empty() && event.queue_name != filter {
                            continue;
                        }
                        if tx.send(Ok(event)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(n, "Event subscriber lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx_stream))))
    }

    type SubscribeLogsStream =
        Pin<Box<dyn Stream<Item = Result<LogEntry, Status>> + Send + 'static>>;

    async fn subscribe_logs(
        &self,
        request: Request<SubscribeLogsRequest>,
    ) -> Result<Response<Self::SubscribeLogsStream>, Status> {
        let req = request.into_inner();
        let (tx, rx) = mpsc::channel(256);
        if req.include_history {
            let logs = self.logs.clone();
            tokio::spawn(async move {
                for line in logs.read(&req.task_run_id, 10_000).await {
                    if tx.send(Ok(log_line_to_proto(line))).await.is_err() {
                        break;
                    }
                }
            });
        }
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

#[tonic::async_trait]
impl worker_service_server::WorkerService for WorkerServiceImpl {
    type SessionStream =
        Pin<Box<dyn Stream<Item = Result<WorkerResponse, Status>> + Send + 'static>>;

    async fn session(
        &self,
        request: Request<Streaming<WorkerRequest>>,
    ) -> Result<Response<Self::SessionStream>, Status> {
        let inbound = request.into_inner();
        let (response_tx, response_rx) = mpsc::channel(256);

        let dispatcher = self.dispatcher.clone();
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            valka_dispatcher::stream::handle_worker_stream(
                dispatcher,
                inbound,
                response_tx,
                shutdown,
            )
            .await;
        });

        let stream = ReceiverStream::new(response_rx).map(Ok);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn checkpoint(
        &self,
        request: Request<CheckpointRequest>,
    ) -> Result<Response<CheckpointResponse>, Status> {
        let req = request.into_inner();
        let output = parse_json("output", &req.output)?.unwrap_or(serde_json::Value::Null);
        self.dispatcher
            .engine()
            .checkpoint(&req.task_id, &req.task_run_id, &req.step, output)
            .await
            .map_err(Status::from)?;
        Ok(Response::new(CheckpointResponse {}))
    }
}

pub async fn serve_grpc(
    addr: SocketAddr,
    engine: Engine,
    dispatcher: DispatcherService,
    event_tx: broadcast::Sender<TaskEvent>,
    node_id: NodeId,
    logs: Arc<LogIngester>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    let api_service = ApiServiceImpl {
        engine: engine.clone(),
        dispatcher: dispatcher.clone(),
        event_tx: event_tx.clone(),
        logs: logs.clone(),
    };

    let worker_service = WorkerServiceImpl {
        dispatcher,
        shutdown: shutdown.clone(),
    };

    let internal_service = InternalServiceImpl {
        engine,
        node_id,
        event_tx,
        logs,
    };

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<api_service_server::ApiServiceServer<ApiServiceImpl>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(valka_proto::valka::v1::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    info!("gRPC server listening on {addr}");

    tonic::transport::Server::builder()
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(10)))
        .http2_keepalive_timeout(Some(std::time::Duration::from_secs(5)))
        .add_service(health_service)
        .add_service(reflection_service)
        .add_service(api_service_server::ApiServiceServer::new(api_service))
        .add_service(worker_service_server::WorkerServiceServer::new(
            worker_service,
        ))
        .add_service(internal_service_server::InternalServiceServer::new(
            internal_service,
        ))
        .serve_with_shutdown(addr, async move {
            let _ = shutdown.changed().await;
        })
        .await?;

    Ok(())
}
