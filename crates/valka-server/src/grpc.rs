use std::net::SocketAddr;
use std::pin::Pin;

use futures::{Stream, StreamExt};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::info;

use valka_core::{NodeId, TaskId, partition_for_task};
use valka_db::DbPool;
use valka_dispatcher::DispatcherService;
use valka_matching::MatchingService;
use valka_matching::partition::TaskEnvelope;
use valka_proto::*;

pub struct ApiServiceImpl {
    pool: DbPool,
    matching: MatchingService,
    dispatcher: DispatcherService,
    event_tx: broadcast::Sender<TaskEvent>,
    _node_id: NodeId,
}

pub struct WorkerServiceImpl {
    dispatcher: DispatcherService,
}

#[tonic::async_trait]
impl api_service_server::ApiService for ApiServiceImpl {
    async fn create_task(
        &self,
        request: Request<CreateTaskRequest>,
    ) -> Result<Response<CreateTaskResponse>, Status> {
        let req = request.into_inner();
        let task_id = TaskId::new();
        let partition = partition_for_task(
            &req.queue_name,
            &task_id.0,
            self.matching.config().num_partitions,
        );

        let input: Option<serde_json::Value> = if req.input.is_empty() {
            None
        } else {
            Some(
                serde_json::from_str(&req.input)
                    .map_err(|e| Status::invalid_argument(format!("Invalid input JSON: {e}")))?,
            )
        };

        let metadata: serde_json::Value = if req.metadata.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&req.metadata)
                .map_err(|e| Status::invalid_argument(format!("Invalid metadata JSON: {e}")))?
        };

        let scheduled_at = if req.scheduled_at.is_empty() {
            None
        } else {
            Some(
                req.scheduled_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .map_err(|e| Status::invalid_argument(format!("Invalid scheduled_at: {e}")))?,
            )
        };

        let max_retries = if req.max_retries == 0 {
            3
        } else {
            req.max_retries
        };
        let timeout_seconds = if req.timeout_seconds == 0 {
            300
        } else {
            req.timeout_seconds
        };

        // Always persist to PG first
        let task_row = valka_db::queries::tasks::create_task(
            &self.pool,
            valka_db::queries::tasks::CreateTaskParams {
                id: task_id.0.clone(),
                queue_name: req.queue_name.clone(),
                task_name: req.task_name.clone(),
                partition_id: partition.0,
                input: input.clone(),
                priority: req.priority,
                max_retries,
                timeout_seconds,
                idempotency_key: if req.idempotency_key.is_empty() {
                    None
                } else {
                    Some(req.idempotency_key.clone())
                },
                metadata: metadata.clone(),
                scheduled_at,
            },
        )
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e
                && db_err.constraint() == Some("idx_tasks_idempotency")
            {
                return Status::already_exists("Task with this idempotency key already exists");
            }
            Status::internal(format!("Database error: {e}"))
        })?;

        valka_core::metrics::record_task_created(&req.queue_name);

        // Emit task created event
        let event = TaskEvent {
            event_id: uuid::Uuid::now_v7().to_string(),
            task_id: task_id.0.clone(),
            queue_name: req.queue_name.clone(),
            previous_status: 0,
            new_status: 1, // PENDING
            worker_id: String::new(),
            node_id: String::new(),
            attempt_number: 0,
            error_message: String::new(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        };
        let _ = self.event_tx.send(event);

        // Try sync match (hot path)
        if scheduled_at.is_none() {
            let envelope = TaskEnvelope {
                task_id: task_id.0.clone(),
                task_run_id: String::new(),
                queue_name: req.queue_name.clone(),
                task_name: req.task_name.clone(),
                input: input.map(|v| v.to_string()),
                attempt_number: 1,
                timeout_seconds,
                metadata: metadata.to_string(),
                priority: req.priority,
            };

            // Fire and forget the sync match - if it fails, TaskReader will pick it up
            let _ = self
                .matching
                .offer_task(&req.queue_name, partition, envelope);
        }

        Ok(Response::new(CreateTaskResponse {
            task: Some(task_row_to_proto(task_row)),
        }))
    }

    async fn get_task(
        &self,
        request: Request<GetTaskRequest>,
    ) -> Result<Response<GetTaskResponse>, Status> {
        let req = request.into_inner();
        let task = valka_db::queries::tasks::get_task(&self.pool, &req.task_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {e}")))?
            .ok_or_else(|| Status::not_found(format!("Task not found: {}", req.task_id)))?;

        Ok(Response::new(GetTaskResponse {
            task: Some(task_row_to_proto(task)),
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
            proto_status_to_str(req.status)
        };

        let (limit, offset) = if let Some(ref p) = req.pagination {
            let offset: i64 = p.page_token.parse().unwrap_or(0);
            (p.page_size as i64, offset)
        } else {
            (50, 0)
        };

        let tasks = valka_db::queries::tasks::list_tasks(
            &self.pool,
            queue_name,
            status_filter,
            limit,
            offset,
        )
        .await
        .map_err(|e| Status::internal(format!("Database error: {e}")))?;

        let next_token = if tasks.len() as i64 == limit {
            (offset + limit).to_string()
        } else {
            String::new()
        };

        Ok(Response::new(ListTasksResponse {
            tasks: tasks.into_iter().map(task_row_to_proto).collect(),
            next_page_token: next_token,
        }))
    }

    async fn cancel_task(
        &self,
        request: Request<CancelTaskRequest>,
    ) -> Result<Response<CancelTaskResponse>, Status> {
        let req = request.into_inner();
        let task = valka_db::queries::tasks::cancel_task_any(&self.pool, &req.task_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {e}")))?
            .ok_or_else(|| {
                Status::failed_precondition(format!(
                    "Task {} not found or not in cancellable state",
                    req.task_id
                ))
            })?;

        // Forward cancellation to worker if running
        self.dispatcher.cancel_task_on_worker(&req.task_id).await;

        // Emit cancel event
        let event = TaskEvent {
            event_id: uuid::Uuid::now_v7().to_string(),
            task_id: req.task_id.clone(),
            queue_name: task.queue_name.clone(),
            previous_status: 0,
            new_status: 8, // CANCELLED
            worker_id: String::new(),
            node_id: String::new(),
            attempt_number: 0,
            error_message: String::new(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        };
        let _ = self.event_tx.send(event);

        Ok(Response::new(CancelTaskResponse {
            task: Some(task_row_to_proto(task)),
        }))
    }

    type SubscribeEventsStream =
        Pin<Box<dyn Stream<Item = Result<TaskEvent, Status>> + Send + 'static>>;

    async fn subscribe_events(
        &self,
        _request: Request<SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        let mut rx = self.event_tx.subscribe();
        let (tx, rx_stream) = mpsc::channel(256);

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
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

        // If include_history, fetch from PG first
        if req.include_history {
            let pool = self.pool.clone();
            let run_id = req.task_run_id.clone();
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                if let Ok(logs) =
                    valka_db::queries::task_logs::get_logs_for_run(&pool, &run_id, 10000, None)
                        .await
                {
                    for log in logs {
                        let entry = LogEntry {
                            task_run_id: log.task_run_id,
                            timestamp_ms: log.timestamp_ms,
                            level: str_to_log_level(&log.level),
                            message: log.message,
                            metadata: log.metadata.map(|m| m.to_string()).unwrap_or_default(),
                        };
                        if tx_clone.send(Ok(entry)).await.is_err() {
                            break;
                        }
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
        tokio::spawn(async move {
            valka_dispatcher::stream::handle_worker_stream(dispatcher, inbound, response_tx).await;
        });

        let stream = ReceiverStream::new(response_rx).map(Ok);
        Ok(Response::new(Box::pin(stream)))
    }
}

pub async fn serve_grpc(
    addr: SocketAddr,
    pool: DbPool,
    dispatcher: DispatcherService,
    matching: MatchingService,
    event_tx: broadcast::Sender<TaskEvent>,
    node_id: NodeId,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    let api_service = ApiServiceImpl {
        pool,
        matching,
        dispatcher: dispatcher.clone(),
        event_tx,
        _node_id: node_id,
    };

    let worker_service = WorkerServiceImpl { dispatcher };

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
        .serve_with_shutdown(addr, async move {
            let _ = shutdown.changed().await;
        })
        .await?;

    Ok(())
}

// --- Helper functions ---

fn task_row_to_proto(row: valka_db::queries::tasks::TaskRow) -> TaskMeta {
    TaskMeta {
        id: row.id,
        queue_name: row.queue_name,
        task_name: row.task_name,
        status: str_to_task_status(&row.status),
        priority: row.priority,
        max_retries: row.max_retries,
        attempt_count: row.attempt_count,
        timeout_seconds: row.timeout_seconds,
        idempotency_key: row.idempotency_key.unwrap_or_default(),
        input: row.input.map(|v| v.to_string()).unwrap_or_default(),
        metadata: row.metadata.to_string(),
        output: row.output.map(|v| v.to_string()).unwrap_or_default(),
        error_message: row.error_message.unwrap_or_default(),
        scheduled_at: row.scheduled_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}

fn str_to_task_status(s: &str) -> i32 {
    match s {
        "PENDING" => 1,
        "DISPATCHING" => 2,
        "RUNNING" => 3,
        "COMPLETED" => 4,
        "FAILED" => 5,
        "RETRY" => 6,
        "DEAD_LETTER" => 7,
        "CANCELLED" => 8,
        _ => 0,
    }
}

fn proto_status_to_str(status: i32) -> Option<&'static str> {
    match status {
        1 => Some("PENDING"),
        2 => Some("DISPATCHING"),
        3 => Some("RUNNING"),
        4 => Some("COMPLETED"),
        5 => Some("FAILED"),
        6 => Some("RETRY"),
        7 => Some("DEAD_LETTER"),
        8 => Some("CANCELLED"),
        _ => None,
    }
}

fn str_to_log_level(s: &str) -> i32 {
    match s {
        "DEBUG" => 1,
        "INFO" => 2,
        "WARN" => 3,
        "ERROR" => 4,
        _ => 0,
    }
}
