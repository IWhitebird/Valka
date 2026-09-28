use crate::service::DispatcherService;
use crate::worker_handle::WorkerHandle;
use futures::StreamExt;
use tokio::sync::{mpsc, watch};
use tonic::Streaming;
use tracing::{error, info, warn};
use valka_core::WorkerId;
use valka_proto::{WorkerRequest, WorkerResponse, worker_request, worker_response};

/// Process the bidirectional worker stream. On server shutdown the worker is told to
/// leave and the stream is closed, so the server can stop without waiting for it.
pub async fn handle_worker_stream(
    dispatcher: DispatcherService,
    mut inbound: Streaming<WorkerRequest>,
    response_tx: mpsc::Sender<WorkerResponse>,
    mut shutdown: watch::Receiver<bool>,
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

    let server_stopping = async move {
        let _ = shutdown.wait_for(|s| *s).await;
    };
    tokio::pin!(server_stopping);

    loop {
        let msg = tokio::select! {
            msg = inbound.next() => msg,
            _ = &mut server_stopping => {
                let bye = WorkerResponse {
                    response: Some(worker_response::Response::ServerShutdown(
                        valka_proto::ServerShutdown {
                            reason: "server shutting down".to_string(),
                            drain_seconds: 0,
                        },
                    )),
                };
                let _ = response_tx.send(bye).await;
                info!(worker_id = %worker_id, "Server shutting down; closing worker stream");
                break;
            }
        };
        match msg {
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
                Some(worker_request::Request::SignalAck(ack)) => {
                    dispatcher.handle_signal_ack(&ack).await;
                }
                Some(worker_request::Request::Shutdown(shutdown)) => {
                    // Stop dispatching, but keep the stream: the worker still sends results
                    // for in-flight tasks and waits for their acks, then closes the stream.
                    info!(
                        worker_id = %worker_id,
                        reason = %shutdown.reason,
                        "Worker draining"
                    );
                    match_handle.abort();
                    dispatcher.stop_dispatching(&worker_id);
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
