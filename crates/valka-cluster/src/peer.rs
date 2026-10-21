//! Clients for other nodes' internal listeners: one cached channel per address, the
//! cluster token on every call, optional mTLS, and a small circuit breaker so a dead peer
//! costs one timeout, not one per request.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tracing::debug;
use valka_proto::internal_service_client::InternalServiceClient;
use valka_proto::{ExecuteRequest, PingRequest, ScatterRequest};

use crate::rpc::{Command, Query, Reply, RpcError, ScatterReply, error_from_proto};

/// Consecutive transport failures before a peer's circuit opens.
const FAILURES_TO_OPEN: u32 = 3;
/// How long an open circuit fails calls fast before letting one through.
const OPEN_FOR: Duration = Duration::from_secs(3);

/// The largest gRPC message a node sends or accepts, for every server and client of the
/// cluster (public, worker and internal), given `limits.max_payload_bytes`: a task's input
/// and metadata (up to that much each) plus every checkpoint it may hold, with room for
/// the envelope. tonic's default (4 MiB) is far below that. 0 (no payload limit) means no
/// message limit either.
pub fn max_message_bytes(max_payload_bytes: usize) -> usize {
    if max_payload_bytes == 0 {
        return usize::MAX;
    }
    let checkpoints = valka_engine::MAX_CHECKPOINTS_PER_TASK
        * (valka_engine::MAX_CHECKPOINT_BYTES + valka_engine::MAX_STEP_NAME_LEN + 64);
    max_payload_bytes
        .saturating_mul(2)
        .saturating_add(checkpoints)
        .saturating_add(4 * 1024 * 1024)
}

/// Whether a failed call says the peer could not be reached (and so counts toward its
/// circuit). A message over the size limit, a refused token or a malformed request say
/// nothing about the peer's health: counting them would cut a healthy peer off.
fn unreachable(code: tonic::Code) -> bool {
    use tonic::Code;
    matches!(
        code,
        Code::Unavailable | Code::Cancelled | Code::DeadlineExceeded | Code::Unknown
    )
}

#[derive(Default)]
struct Circuit {
    failures: u32,
    opened_at: Option<Instant>,
}

#[derive(Clone)]
pub struct PeerClients {
    inner: Arc<Inner>,
}

struct Inner {
    me: String,
    token: Option<String>,
    tls: Option<ClientTlsConfig>,
    call_timeout: Duration,
    channels: Mutex<HashMap<String, Channel>>,
    circuits: Mutex<HashMap<String, Circuit>>,
    /// See [`max_message_bytes`].
    max_message_bytes: AtomicUsize,
}

impl PeerClients {
    /// `call_timeout` bounds every call, connection included.
    pub fn new(
        me: &str,
        token: Option<String>,
        tls: Option<ClientTlsConfig>,
        call_timeout: Duration,
    ) -> Self {
        if tls.is_some() {
            crate::security::install_crypto_provider();
        }
        Self {
            inner: Arc::new(Inner {
                me: me.to_string(),
                token: token.filter(|t| !t.is_empty()),
                tls,
                call_timeout,
                channels: Mutex::new(HashMap::new()),
                circuits: Mutex::new(HashMap::new()),
                max_message_bytes: AtomicUsize::new(max_message_bytes(
                    valka_core::LimitsConfig::default().max_payload_bytes,
                )),
            }),
        }
    }

    /// Size the messages of every call to `bytes` (see [`max_message_bytes`]); peers must
    /// use the same.
    pub fn set_max_message_bytes(&self, bytes: usize) {
        self.inner.max_message_bytes.store(bytes, Ordering::Relaxed);
    }

    fn max_message(&self) -> usize {
        self.inner.max_message_bytes.load(Ordering::Relaxed)
    }

    fn channel(&self, addr: &str) -> Result<Channel, RpcError> {
        if let Some(c) = self.inner.channels.lock().get(addr) {
            return Ok(c.clone());
        }
        let scheme = if self.inner.tls.is_some() {
            "https"
        } else {
            "http"
        };
        let mut ep = Endpoint::from_shared(format!("{scheme}://{addr}"))
            .map_err(|e| RpcError::Transport(format!("bad peer address {addr}: {e}")))?
            .connect_timeout(Duration::from_secs(3))
            .timeout(self.inner.call_timeout)
            .http2_keep_alive_interval(Duration::from_secs(10))
            .keep_alive_while_idle(true);
        if let Some(tls) = &self.inner.tls {
            let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
            ep = ep
                .tls_config(tls.clone().domain_name(host.trim_matches(['[', ']'])))
                .map_err(|e| RpcError::Transport(format!("tls config: {e}")))?;
        }
        let ch = ep.connect_lazy();
        self.inner
            .channels
            .lock()
            .insert(addr.to_string(), ch.clone());
        Ok(ch)
    }

    fn client(
        &self,
        addr: &str,
    ) -> Result<
        InternalServiceClient<
            tonic::service::interceptor::InterceptedService<
                Channel,
                impl tonic::service::Interceptor + Clone,
            >,
        >,
        RpcError,
    > {
        let ch = self.channel(addr)?;
        let auth = self
            .inner
            .token
            .as_ref()
            .map(|t| MetadataValue::try_from(format!("Bearer {t}")))
            .transpose()
            .map_err(|e| RpcError::Transport(format!("bad token: {e}")))?;
        let max = self.max_message();
        Ok(
            InternalServiceClient::with_interceptor(ch, move |mut req: tonic::Request<()>| {
                if let Some(a) = &auth {
                    req.metadata_mut().insert("authorization", a.clone());
                }
                Ok(req)
            })
            .max_decoding_message_size(max)
            .max_encoding_message_size(max),
        )
    }

    fn admit(&self, addr: &str) -> Result<(), RpcError> {
        let mut c = self.inner.circuits.lock();
        let circuit = c.entry(addr.to_string()).or_default();
        if let Some(at) = circuit.opened_at {
            if at.elapsed() < OPEN_FOR {
                return Err(RpcError::Transport(format!("{addr}: circuit open")));
            }
            // Half-open: let this call probe the peer.
            circuit.opened_at = None;
            circuit.failures = FAILURES_TO_OPEN - 1;
        }
        Ok(())
    }

    fn record(&self, addr: &str, ok: bool) {
        let mut c = self.inner.circuits.lock();
        let circuit = c.entry(addr.to_string()).or_default();
        if ok {
            *circuit = Circuit::default();
        } else {
            circuit.failures += 1;
            if circuit.failures >= FAILURES_TO_OPEN && circuit.opened_at.is_none() {
                debug!(addr, "peer circuit open");
                circuit.opened_at = Some(Instant::now());
                // A broken channel is rebuilt on the next attempt.
                self.inner.channels.lock().remove(addr);
            }
        }
    }

    fn transport(&self, addr: &str, s: tonic::Status) -> RpcError {
        if unreachable(s.code()) {
            self.record(addr, false);
        }
        RpcError::Transport(format!("{addr}: {}: {}", s.code(), s.message()))
    }

    /// Run `cmd` on the node at `addr`.
    pub async fn execute(&self, addr: &str, cmd: &Command) -> Result<Reply, RpcError> {
        self.admit(addr)?;
        let body = serde_json::to_vec(cmd).map_err(|e| RpcError::Transport(e.to_string()))?;
        let resp = self
            .client(addr)?
            .execute(ExecuteRequest {
                command: body,
                from_node: self.inner.me.clone(),
            })
            .await
            .map_err(|s| self.transport(addr, s))?
            .into_inner();
        self.record(addr, true);
        if let Some(e) = resp.error {
            return Err(error_from_proto(e));
        }
        serde_json::from_slice(&resp.reply)
            .map_err(|e| RpcError::Transport(format!("{addr}: bad reply: {e}")))
    }

    /// Ask the node at `addr` about its own shards.
    pub async fn scatter(&self, addr: &str, q: &Query) -> Result<ScatterReply, RpcError> {
        self.admit(addr)?;
        let body = serde_json::to_vec(q).map_err(|e| RpcError::Transport(e.to_string()))?;
        let resp = self
            .client(addr)?
            .scatter(ScatterRequest {
                query: body,
                from_node: self.inner.me.clone(),
            })
            .await
            .map_err(|s| self.transport(addr, s))?
            .into_inner();
        self.record(addr, true);
        if let Some(e) = resp.error {
            return Err(error_from_proto(e));
        }
        serde_json::from_slice(&resp.reply)
            .map_err(|e| RpcError::Transport(format!("{addr}: bad reply: {e}")))
    }

    /// The task events of the node at `addr` (its own shards only).
    pub async fn events(
        &self,
        addr: &str,
        from: &str,
    ) -> Result<tonic::Streaming<valka_proto::TaskEvent>, RpcError> {
        self.admit(addr)?;
        // A long-lived stream: no per-call timeout on this channel.
        let ch = {
            let scheme = if self.inner.tls.is_some() {
                "https"
            } else {
                "http"
            };
            let mut ep = Endpoint::from_shared(format!("{scheme}://{addr}"))
                .map_err(|e| RpcError::Transport(format!("bad peer address {addr}: {e}")))?
                .connect_timeout(Duration::from_secs(3))
                .http2_keep_alive_interval(Duration::from_secs(10))
                .keep_alive_while_idle(true);
            if let Some(tls) = &self.inner.tls {
                let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
                ep = ep
                    .tls_config(tls.clone().domain_name(host.trim_matches(['[', ']'])))
                    .map_err(|e| RpcError::Transport(format!("tls config: {e}")))?;
            }
            ep.connect_lazy()
        };
        let auth = self
            .inner
            .token
            .as_ref()
            .map(|t| MetadataValue::try_from(format!("Bearer {t}")))
            .transpose()
            .map_err(|e| RpcError::Transport(format!("bad token: {e}")))?;
        let max = self.max_message();
        let mut client =
            InternalServiceClient::with_interceptor(ch, move |mut req: tonic::Request<()>| {
                if let Some(a) = &auth {
                    req.metadata_mut().insert("authorization", a.clone());
                }
                Ok(req)
            })
            .max_decoding_message_size(max)
            .max_encoding_message_size(max);
        let s = client
            .events(valka_proto::EventsRequest {
                from_node: from.to_string(),
            })
            .await
            .map_err(|s| self.transport(addr, s))?
            .into_inner();
        self.record(addr, true);
        Ok(s)
    }

    /// The node id answering at `addr`.
    pub async fn ping(&self, addr: &str) -> Result<valka_proto::PingResponse, RpcError> {
        self.admit(addr)?;
        let r = self
            .client(addr)?
            .ping(PingRequest {
                node_id: self.inner.me.clone(),
            })
            .await
            .map_err(|s| self.transport(addr, s))?
            .into_inner();
        self.record(addr, true);
        Ok(r)
    }
}
