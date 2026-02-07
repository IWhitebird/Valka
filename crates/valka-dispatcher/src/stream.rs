use crate::service::DispatcherService;
use crate::worker_handle::WorkerHandle;
use futures::StreamExt;
use tokio::sync::mpsc;
use tonic::Streaming;
use tracing::{error, info, warn};
use valka_core::WorkerId;
use valka_proto::{WorkerRequest, WorkerResponse, worker_request, worker_response};

/// Process the bidirectional worker stream
pub async fn handle_worker_stream(
    dispatcher: DispatcherService,
    mut inbound: Streaming<WorkerRequest>,
    response_tx: mpsc::Sender<WorkerResponse>,
) {
    // First message must be WorkerHello
    let hello = match inbound.next().await {
        Some(Ok(msg)) => match msg.request {
            Some(worker_request::Request::Hello(hello)) => hello,
            _ => {
                error!("First message must be WorkerHello");
                return;
            }
        },
        _ => {
            error!("Worker stream closed before hello");
            return;
        }
    };

    let worker_id = if hello.worker_id.is_empty() {
        WorkerId::new()
    } else {
        WorkerId(hello.worker_id.clone())
    };

    info!(
        worker_id = %worker_id,
        worker_name = %hello.worker_name,
        queues = ?hello.queues,
        concurrency = hello.concurrency,
        "Worker connected"
    );

    // Register worker
    let handle = WorkerHandle::new(
        worker_id.clone(),
        hello.worker_name,
        hello.queues.clone(),
        hello.concurrency,
        response_tx.clone(),
        hello.metadata,
    );

    dispatcher.register_worker(handle).await;

    // Start background task matching loop for this worker
    let dispatcher_clone = dispatcher.clone();
    let worker_id_clone = worker_id.clone();
    let queues = hello.queues.clone();
    let match_handle = tokio::spawn(async move {
        dispatcher_clone
            .run_worker_match_loop(worker_id_clone, queues)
            .await;
    });

    // Process incoming messages
    loop {
        match inbound.next().await {
            Some(Ok(msg)) => match msg.request {
                Some(worker_request::Request::TaskResult(result)) => {
                    dispatcher.handle_task_result(&worker_id, result).await;
                }
                Some(worker_request::Request::Heartbeat(hb)) => {
                    dispatcher.handle_heartbeat(&worker_id, hb).await;
                    let ack = WorkerResponse {
                        response: Some(worker_response::Response::HeartbeatAck(
                            valka_proto::HeartbeatAck {
                                server_timestamp_ms: chrono::Utc::now().timestamp_millis(),
                            },
                        )),
                    };
                    if response_tx.send(ack).await.is_err() {
                        break;
                    }
                }
                Some(worker_request::Request::LogBatch(batch)) => {
                    dispatcher.handle_log_batch(&worker_id, batch).await;
                }
                Some(worker_request::Request::Shutdown(shutdown)) => {
                    info!(
                        worker_id = %worker_id,
                        reason = %shutdown.reason,
                        "Worker graceful shutdown"
                    );
                    break;
                }
                None => {
                    warn!(worker_id = %worker_id, "Empty worker request");
                }
                _ => {}
            },
            Some(Err(e)) => {
                warn!(worker_id = %worker_id, error = %e, "Worker stream error");
                break;
            }
            None => {
                info!(worker_id = %worker_id, "Worker stream closed");
                break;
            }
        }
    }

    // Cleanup
    match_handle.abort();
    dispatcher.deregister_worker(&worker_id).await;
}
