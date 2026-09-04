use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::debug;

use crate::convert::log_line_to_proto;
use valka_core::NodeId;
use valka_engine::{Engine, LogIngester};
use valka_proto::*;

pub struct InternalServiceImpl {
    pub engine: Engine,
    pub node_id: NodeId,
    pub event_tx: broadcast::Sender<TaskEvent>,
    pub logs: Arc<LogIngester>,
}

#[tonic::async_trait]
impl internal_service_server::InternalService for InternalServiceImpl {
    /// Phase 1: a single node owns every shard, so a forwarded task is simply re-offered
    /// from the local pending index. Phase 2 replaces this with shard-owner routing.
    async fn forward_task(
        &self,
        request: Request<ForwardTaskRequest>,
    ) -> Result<Response<ForwardTaskResponse>, Status> {
        let req = request.into_inner();
        debug!(task_id = %req.task_id, queue = %req.queue_name, "Received forwarded task");
        let accepted = self.engine.get_task(&req.task_id).is_some();
        if accepted {
            self.engine.unoffer(&req.task_id);
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
        let logs = self.logs.clone();
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            for line in logs.read(&req.task_run_id, 10_000).await {
                if tx.send(Ok(log_line_to_proto(line))).await.is_err() {
                    break;
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
