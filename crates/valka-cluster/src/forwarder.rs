use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tonic::transport::Channel;
use tracing::{debug, warn};

use valka_proto::internal_service_client::InternalServiceClient;
use valka_proto::{ForwardEventRequest, ForwardTaskRequest, LogEntry, RelayLogsRequest, TaskEvent};

const FAILURE_THRESHOLD: u32 = 3;
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const FORWARD_RETRY_DELAY: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

pub struct NodeCircuit {
    pub state: CircuitState,
    pub failure_count: u32,
    pub last_failure: Option<Instant>,
}

impl Default for NodeCircuit {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            last_failure: None,
        }
    }
}

/// gRPC client for inter-node RPCs, with connection caching and circuit breaker.
#[derive(Clone)]
pub struct NodeForwarder {
    channels: Arc<RwLock<HashMap<String, InternalServiceClient<Channel>>>>,
    circuits: Arc<RwLock<HashMap<String, NodeCircuit>>>,
}

impl NodeForwarder {
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            circuits: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn get_client(&self, addr: &str) -> anyhow::Result<InternalServiceClient<Channel>> {
        // Check cache first
        {
            let cache = self.channels.read().await;
            if let Some(client) = cache.get(addr) {
                return Ok(client.clone());
            }
        }

        // Create new connection
        let endpoint = format!("http://{addr}");
        let channel = Channel::from_shared(endpoint)?
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .connect()
            .await?;

        let client = InternalServiceClient::new(channel);

        // Cache it
        {
            let mut cache = self.channels.write().await;
            cache.insert(addr.to_string(), client.clone());
        }

        Ok(client)
    }

    /// Check if a call to the given addr is allowed by the circuit breaker.
    /// Returns true if allowed (Closed or HalfOpen), false if Open.
    async fn check_circuit(&self, addr: &str) -> bool {
        let mut circuits = self.circuits.write().await;
        let circuit = circuits.entry(addr.to_string()).or_default();

        match circuit.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => {
                // Check if recovery timeout has elapsed
                if let Some(last_failure) = circuit.last_failure {
                    if last_failure.elapsed() >= RECOVERY_TIMEOUT {
                        circuit.state = CircuitState::HalfOpen;
                        debug!(addr = addr, "Circuit breaker transitioning to half-open");
                        true
                    } else {
                        false
                    }
                } else {
                    // No last_failure recorded, shouldn't happen but allow
                    circuit.state = CircuitState::Closed;
                    true
                }
            }
        }
    }

    /// Record a successful call, resetting the circuit to Closed.
    async fn record_success(&self, addr: &str) {
        let mut circuits = self.circuits.write().await;
        if let Some(circuit) = circuits.get_mut(addr) {
            if circuit.state != CircuitState::Closed {
                debug!(addr = addr, "Circuit breaker reset to closed");
            }
            circuit.state = CircuitState::Closed;
            circuit.failure_count = 0;
            circuit.last_failure = None;
        }
    }

    /// Record a failed call. Opens the circuit if failure threshold is reached.
    async fn record_failure(&self, addr: &str) {
        let mut circuits = self.circuits.write().await;
        let circuit = circuits.entry(addr.to_string()).or_default();
        circuit.failure_count += 1;
        circuit.last_failure = Some(Instant::now());

        if circuit.failure_count >= FAILURE_THRESHOLD {
            if circuit.state != CircuitState::Open {
                warn!(
                    addr = addr,
                    failures = circuit.failure_count,
                    "Circuit breaker opened for node"
                );
                valka_core::metrics::record_forward_circuit_open(addr);
            }
            circuit.state = CircuitState::Open;
        }
    }

    /// Forward a task to the owning node for sync matching.
    /// Includes 1 retry with 200ms delay and circuit breaker protection.
    pub async fn forward_task(
        &self,
        addr: &str,
        task_id: &str,
        queue_name: &str,
        partition_id: i32,
    ) -> anyhow::Result<bool> {
        // Check circuit breaker
        if !self.check_circuit(addr).await {
            return Err(anyhow::anyhow!("Circuit breaker open for node {addr}"));
        }

        // First attempt
        let first_err = match self
            .do_forward_task(addr, task_id, queue_name, partition_id)
            .await
        {
            Ok(accepted) => {
                self.record_success(addr).await;
                return Ok(accepted);
            }
            Err(e) => {
                self.record_failure(addr).await;
                e
            }
        };

        // Retry once after delay (only if circuit isn't now open)
        if self.check_circuit(addr).await {
            tokio::time::sleep(FORWARD_RETRY_DELAY).await;

            match self
                .do_forward_task(addr, task_id, queue_name, partition_id)
                .await
            {
                Ok(accepted) => {
                    self.record_success(addr).await;
                    return Ok(accepted);
                }
                Err(retry_err) => {
                    self.record_failure(addr).await;
                    debug!(
                        addr = addr,
                        task_id = task_id,
                        error = %retry_err,
                        "Forward task retry also failed"
                    );
                }
            }
        }

        Err(first_err)
    }

    /// Internal: perform the actual gRPC forward_task call.
    async fn do_forward_task(
        &self,
        addr: &str,
        task_id: &str,
        queue_name: &str,
        partition_id: i32,
    ) -> anyhow::Result<bool> {
        let mut client = self.get_client(addr).await?;
        let resp = client
            .forward_task(ForwardTaskRequest {
                task_id: task_id.to_string(),
                queue_name: queue_name.to_string(),
                partition_id,
            })
            .await?;
        debug!(
            task_id = task_id,
            addr = addr,
            accepted = resp.get_ref().accepted,
            "Task forwarded"
        );
        Ok(resp.get_ref().accepted)
    }

    /// Forward a task event to a peer node (best-effort, no retry).
    pub async fn forward_event(&self, addr: &str, event: TaskEvent) -> anyhow::Result<()> {
        let mut client = self.get_client(addr).await?;
        client
            .forward_event(ForwardEventRequest { event: Some(event) })
            .await?;
        Ok(())
    }

    /// Relay logs from a peer node for the given task_run_id (best-effort, no retry).
    pub async fn relay_logs(
        &self,
        addr: &str,
        task_run_id: &str,
    ) -> anyhow::Result<tonic::Streaming<LogEntry>> {
        let mut client = self.get_client(addr).await?;
        let resp = client
            .relay_logs(RelayLogsRequest {
                task_run_id: task_run_id.to_string(),
            })
            .await?;
        Ok(resp.into_inner())
    }

    /// Evict a cached channel and circuit state for a node (e.g., on NodeLeft).
    pub async fn remove_node(&self, addr: &str) {
        let mut cache = self.channels.write().await;
        cache.remove(addr);
        drop(cache);

        let mut circuits = self.circuits.write().await;
        circuits.remove(addr);
    }

    /// Get the current circuit state for a node address (for testing/monitoring).
    pub async fn get_circuit_state(&self, addr: &str) -> CircuitState {
        let circuits = self.circuits.read().await;
        circuits
            .get(addr)
            .map(|c| c.state)
            .unwrap_or(CircuitState::Closed)
    }
}

impl Default for NodeForwarder {
    fn default() -> Self {
        Self::new()
    }
}
