use std::pin::Pin;

use futures::Stream;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::debug;

use valka_core::{NodeId, PartitionId};
use valka_db::DbPool;
use valka_matching::MatchingService;
use valka_matching::partition::TaskEnvelope;
use valka_proto::*;

pub struct InternalServiceImpl {
    pub pool: DbPool,
    pub matching: MatchingService,
    pub node_id: NodeId,
    pub event_tx: broadcast::Sender<TaskEvent>,
}

#[tonic::async_trait]
impl internal_service_server::InternalService for InternalServiceImpl {
    async fn forward_task(
        &self,
        request: Request<ForwardTaskRequest>,
    ) -> Result<Response<ForwardTaskResponse>, Status> {
        let req = request.into_inner();
        debug!(
            task_id = %req.task_id,
            queue = %req.queue_name,
            partition = req.partition_id,
            "Received forwarded task"
        );

        // Read the full task from PG (task was already persisted by originating node)
        let task_row = valka_db::queries::tasks::get_task(&self.pool, &req.task_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {e}")))?
            .ok_or_else(|| {
                Status::not_found(format!("Forwarded task not found: {}", req.task_id))
            })?;

        // Build TaskEnvelope from the task row
        let envelope = TaskEnvelope {
            task_id: task_row.id.clone(),
            task_run_id: String::new(),
            queue_name: task_row.queue_name.clone(),
            task_name: task_row.task_name.clone(),
            input: task_row.input.map(|v| v.to_string()),
            attempt_number: task_row.attempt_count + 1,
            timeout_seconds: task_row.timeout_seconds,
            metadata: task_row.metadata.to_string(),
            priority: task_row.priority,
        };

        // Try sync match locally (on the owning node)
        let partition = PartitionId(req.partition_id);
        let accepted = self
            .matching
            .offer_task(&req.queue_name, partition, envelope)
            .is_ok();

        if accepted {
            debug!(task_id = %req.task_id, "Forwarded task accepted via sync match");
        }

        Ok(Response::new(ForwardTaskResponse { accepted }))
    }

    async fn forward_event(
        &self,
        request: Request<ForwardEventRequest>,
    ) -> Result<Response<ForwardEventResponse>, Status> {
        let req = request.into_inner();
        if let Some(event) = req.event {
            let _ = self.event_tx.send(event);
        }
        Ok(Response::new(ForwardEventResponse {}))
    }

    type RelayLogsStream = Pin<Box<dyn Stream<Item = Result<LogEntry, Status>> + Send + 'static>>;

    async fn relay_logs(
        &self,
        request: Request<RelayLogsRequest>,
    ) -> Result<Response<Self::RelayLogsStream>, Status> {
        let req = request.into_inner();
        let pool = self.pool.clone();

        let (tx, rx) = mpsc::channel(256);

        tokio::spawn(async move {
            match valka_db::queries::task_logs::get_logs_for_run(
                &pool,
                &req.task_run_id,
                10000,
                None,
            )
            .await
            {
                Ok(logs) => {
                    for log in logs {
                        let entry = LogEntry {
                            task_run_id: log.task_run_id,
                            timestamp_ms: log.timestamp_ms,
                            level: str_to_log_level(&log.level),
                            message: log.message,
                            metadata: log.metadata.map(|m| m.to_string()).unwrap_or_default(),
                        };
                        if tx.send(Ok(entry)).await.is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("Database error: {e}"))))
                        .await;
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn ping(&self, _request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            node_id: self.node_id.0.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        }))
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
