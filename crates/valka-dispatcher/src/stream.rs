use std::sync::Arc;

use crate::service::DispatcherService;
use crate::worker_handle::{Latch, WorkerHandle};
use futures::StreamExt;
use tokio::sync::{Semaphore, mpsc, watch};
use tonic::Streaming;
use tracing::{error, info, warn};
use valka_core::WorkerId;
use valka_proto::{WorkerRequest, WorkerResponse, worker_request, worker_response};

/// Results and heartbeats one stream may have in progress at once. Each result waits for its
/// group commit, so handling them one after another would cap a worker at one result per
/// commit window; past this many the stream stops reading until some finish.
const MAX_IN_PROGRESS: usize = 1024;

/// Closes the connection's latch however the handler ends, so the response stream (and
/// with it the worker's connection) ends too.
struct CloseOnDrop(Latch);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Process the bidirectional worker stream. On server shutdown the worker is told to
/// leave and the stream is closed, so the server can stop without waiting for it.
///
/// `closer` ends this connection: the transport ends the response stream once it is
/// closed, and closing it from elsewhere (the heartbeat checker, a reconnect of the same
/// worker) stops this handler. The handler closes it when it returns.
pub async fn handle_worker_stream(
    dispatcher: DispatcherService,
    mut inbound: Streaming<WorkerRequest>,
    response_tx: mpsc::Sender<WorkerResponse>,
    mut shutdown: watch::Receiver<bool>,
    closer: Latch,
) {
    let _close = CloseOnDrop(closer.clone());
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
    )
    .with_closer(closer.clone());
    let conn_id = handle.conn_id;

    dispatcher.register_worker(handle).await;
    // Runs the worker kept running across a reconnect: reattach, adopt or cancel each,
    // before it is offered new work (its slots are busy with them).
    if !hello.running.is_empty() {
        dispatcher
            .reattach_running(&worker_id, &hello.running, &response_tx)
            .await;
    }
    dispatcher.connection_ready(&worker_id, conn_id);

    // Start background task matching loop for this worker. It ends by itself when this
    // connection drains or goes away, handing back whatever it already received.
    let dispatcher_clone = dispatcher.clone();
    let worker_id_clone = worker_id.clone();
    let queues = hello.queues.clone();
    tokio::spawn(async move {
        dispatcher_clone
            .run_worker_match_loop(worker_id_clone, queues)
            .await;
    });

    let in_progress = Arc::new(Semaphore::new(MAX_IN_PROGRESS));

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
            _ = closer.closed() => {
                info!(worker_id = %worker_id, "Worker connection closed by the server");
                break;
            }
        };
        match msg {
            Some(Ok(msg)) => match msg.request {
                Some(worker_request::Request::TaskResult(result)) => {
                    // Results are idempotent and acknowledged one by one, so they need no
                    // order; a result whose handling outlives the stream is still applied.
                    let Ok(permit) = in_progress.clone().acquire_owned().await else {
                        break;
                    };
                    let (dispatcher, worker_id) = (dispatcher.clone(), worker_id.clone());
                    tokio::spawn(async move {
                        dispatcher.handle_task_result(&worker_id, result).await;
                        drop(permit);
                    });
                }
                Some(worker_request::Request::Heartbeat(hb)) => {
                    let Ok(permit) = in_progress.clone().acquire_owned().await else {
                        break;
                    };
                    let (dispatcher, worker_id) = (dispatcher.clone(), worker_id.clone());
                    let response_tx = response_tx.clone();
                    tokio::spawn(async move {
                        dispatcher.handle_heartbeat(&worker_id, hb).await;
                        let ack = WorkerResponse {
                            response: Some(worker_response::Response::HeartbeatAck(
                                valka_proto::HeartbeatAck {
                                    server_timestamp_ms: chrono::Utc::now().timestamp_millis(),
                                },
                            )),
                        };
                        let _ = response_tx.send(ack).await;
                        drop(permit);
                    });
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
                    // The match loop hands back what it holds and ends.
                    info!(
                        worker_id = %worker_id,
                        reason = %shutdown.reason,
                        "Worker draining"
                    );
                    dispatcher.stop_dispatching_connection(&worker_id, conn_id);
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

    // Cleanup: this connection only (the worker may already have reconnected).
    closer.close();
    dispatcher.deregister_connection(&worker_id, conn_id).await;
}
