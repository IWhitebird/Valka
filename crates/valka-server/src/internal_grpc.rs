//! The internal listener: other nodes call `Execute` (commands for shards this node owns)
//! and `Scatter` (questions about this node's shards). Never exposed publicly; every call
//! carries the cluster token, mTLS is optional (PHASE2 §11.6).

use std::net::SocketAddr;

use tokio::sync::watch;
use tonic::{Request, Response, Status};
use tracing::info;

use crate::api::Api;
use valka_cluster::rpc::error_to_proto;
use valka_cluster::{Command, Query};
use valka_core::ServerError;
use valka_proto::*;

pub struct InternalServiceImpl {
    pub api: Api,
    /// This node's own task events (not the cluster-wide channel: no relay loops).
    pub local_events: tokio::sync::broadcast::Sender<TaskEvent>,
}

impl InternalServiceImpl {
    fn hint(&self, e: &ServerError) -> Option<String> {
        match e {
            ServerError::NotOwner(s) => self.api.owner_hint(valka_core::ShardId(*s)),
            _ => None,
        }
    }
}

#[tonic::async_trait]
impl internal_service_server::InternalService for InternalServiceImpl {
    async fn execute(
        &self,
        request: Request<ExecuteRequest>,
    ) -> Result<Response<ExecuteResponse>, Status> {
        let req = request.into_inner();
        let cmd: Command = serde_json::from_slice(&req.command)
            .map_err(|e| Status::invalid_argument(format!("bad command: {e}")))?;
        let resp = match self.api.execute_local(cmd).await {
            Ok(reply) => ExecuteResponse {
                reply: serde_json::to_vec(&reply).map_err(|e| Status::internal(e.to_string()))?,
                error: None,
            },
            Err(e) => ExecuteResponse {
                reply: Vec::new(),
                error: Some(error_to_proto(&e, self.hint(&e))),
            },
        };
        Ok(Response::new(resp))
    }

    async fn scatter(
        &self,
        request: Request<ScatterRequest>,
    ) -> Result<Response<ScatterResponse>, Status> {
        let req = request.into_inner();
        let q: Query = serde_json::from_slice(&req.query)
            .map_err(|e| Status::invalid_argument(format!("bad query: {e}")))?;
        let resp = match self.api.scatter_local(q).await {
            Ok(reply) => ScatterResponse {
                reply: serde_json::to_vec(&reply).map_err(|e| Status::internal(e.to_string()))?,
                error: None,
            },
            Err(e) => ScatterResponse {
                reply: Vec::new(),
                error: Some(error_to_proto(&e, None)),
            },
        };
        Ok(Response::new(resp))
    }

    type EventsStream =
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<TaskEvent, Status>> + Send + 'static>>;

    async fn events(
        &self,
        _request: Request<EventsRequest>,
    ) -> Result<Response<Self::EventsStream>, Status> {
        let mut rx = self.local_events.subscribe();
        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(ev) => yield Ok(ev),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn ping(&self, _request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            node_id: self.api.node_id().to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            incarnation: self.api.engine().incarnation(),
            protocol: valka_wal::lease::PROTOCOL,
        }))
    }
}

/// Serve the internal listener until `shutdown`. `listener` lets tests bind port 0.
pub async fn serve_internal(
    listener: tokio::net::TcpListener,
    api: Api,
    local_events: tokio::sync::broadcast::Sender<TaskEvent>,
    token: Option<String>,
    tls: Option<tonic::transport::ServerTlsConfig>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    let addr: SocketAddr = listener.local_addr()?;
    let svc = internal_service_server::InternalServiceServer::with_interceptor(
        InternalServiceImpl { api, local_events },
        valka_cluster::security::token_check(token),
    );
    let mut builder = tonic::transport::Server::builder()
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(10)))
        .http2_keepalive_timeout(Some(std::time::Duration::from_secs(5)));
    if let Some(tls) = tls {
        valka_cluster::security::install_crypto_provider();
        builder = builder.tls_config(tls)?;
    }
    info!(%addr, "internal listener started");
    builder
        .add_service(svc)
        .serve_with_incoming_shutdown(
            tokio_stream::wrappers::TcpListenerStream::new(listener),
            async move {
                let _ = shutdown.wait_for(|s| *s).await;
            },
        )
        .await?;
    Ok(())
}
