use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chitchat::transport::UdpTransport;
use chitchat::{ChitchatConfig, ChitchatHandle, ChitchatId, spawn_chitchat};
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn};
use valka_core::{GossipConfig, NodeId};

use crate::events::ClusterEvent;
use crate::ring::HashRing;

/// Manages cluster membership via chitchat gossip protocol.
/// In single-node mode, this owns all partitions and has no gossip.
pub struct ClusterManager {
    node_id: NodeId,
    ring: Arc<RwLock<HashRing>>,
    members: Arc<RwLock<HashSet<String>>>,
    grpc_addrs: Arc<RwLock<HashMap<String, String>>>,
    event_tx: broadcast::Sender<ClusterEvent>,
    num_partitions: i32,
    chitchat_handle: Option<ChitchatHandle>,
}

impl ClusterManager {
    pub fn new_single_node(node_id: NodeId, num_partitions: i32) -> Self {
        let mut ring = HashRing::new();
        ring.add_node(&node_id.0);

        let mut members = HashSet::new();
        members.insert(node_id.0.clone());

        let (event_tx, _) = broadcast::channel(256);

        Self {
            node_id,
            ring: Arc::new(RwLock::new(ring)),
            members: Arc::new(RwLock::new(members)),
            grpc_addrs: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            num_partitions,
            chitchat_handle: None,
        }
    }

    pub async fn new_clustered(
        node_id: NodeId,
        num_partitions: i32,
        gossip_config: &GossipConfig,
        grpc_addr: &str,
    ) -> anyhow::Result<Self> {
        let listen_addr: std::net::SocketAddr = gossip_config.listen_addr.parse()?;
        let raw_advertise = gossip_config
            .advertise_addr
            .as_deref()
            .unwrap_or(&gossip_config.listen_addr);
        // Resolve hostname to IP (supports both "1.2.3.4:7280" and "hostname:7280")
        let advertise_addr: std::net::SocketAddr = tokio::net::lookup_host(raw_advertise)
            .await?
            .next()
            .ok_or_else(|| anyhow::anyhow!("failed to resolve advertise_addr: {raw_advertise}"))?;

        let generation_id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;

        let chitchat_id = ChitchatId::new(node_id.0.clone(), generation_id, advertise_addr);

        let config = ChitchatConfig {
            chitchat_id,
            cluster_id: gossip_config.cluster_id.clone(),
            gossip_interval: Duration::from_millis(500),
            listen_addr,
            seed_nodes: gossip_config.seed_nodes.clone(),
            failure_detector_config: Default::default(),
            marked_for_deletion_grace_period: Duration::from_secs(3600),
            catchup_callback: None,
            extra_liveness_predicate: None,
        };

        let handle = spawn_chitchat(
            config,
            vec![("grpc_addr".to_string(), grpc_addr.to_string())],
            &UdpTransport,
        )
        .await?;

        let mut ring = HashRing::new();
        ring.add_node(&node_id.0);

        let mut members = HashSet::new();
        members.insert(node_id.0.clone());

        let mut grpc_addrs = HashMap::new();
        grpc_addrs.insert(node_id.0.clone(), grpc_addr.to_string());

        let (event_tx, _) = broadcast::channel(256);

        let manager = Self {
            node_id,
            ring: Arc::new(RwLock::new(ring)),
            members: Arc::new(RwLock::new(members)),
            grpc_addrs: Arc::new(RwLock::new(grpc_addrs)),
            event_tx,
            num_partitions,
            chitchat_handle: Some(handle),
        };

        // Spawn background membership watcher
        manager.spawn_membership_watcher();

        info!(
            node_id = %manager.node_id,
            listen_addr = %listen_addr,
            "Cluster gossip started"
        );

        Ok(manager)
    }

    fn spawn_membership_watcher(&self) {
        let handle = self.chitchat_handle.as_ref().expect("chitchat handle");
        let chitchat = handle.chitchat();
        let ring = self.ring.clone();
        let members = self.members.clone();
        let grpc_addrs = self.grpc_addrs.clone();
        let event_tx = self.event_tx.clone();
        let self_node_id = self.node_id.clone();

        tokio::spawn(async move {
            let mut watcher = {
                let guard = chitchat.lock().await;
                guard.live_nodes_watcher()
            };

            loop {
                if watcher.changed().await.is_err() {
                    info!("Membership watcher channel closed, stopping");
                    break;
                }

                let live_nodes = watcher.borrow().clone();
                let mut new_members = HashSet::new();
                let mut new_grpc_addrs = HashMap::new();

                // Always include self
                new_members.insert(self_node_id.0.clone());
                // Self grpc_addr is already set during init; keep it
                {
                    let addrs = grpc_addrs.read().await;
                    if let Some(addr) = addrs.get(&self_node_id.0) {
                        new_grpc_addrs.insert(self_node_id.0.clone(), addr.clone());
                    }
                }

                for (chitchat_id, node_state) in &live_nodes {
                    let nid = &chitchat_id.node_id;
                    new_members.insert(nid.clone());
                    if let Some(addr) = node_state.get("grpc_addr") {
                        new_grpc_addrs.insert(nid.clone(), addr.to_string());
                    }
                }

                // Diff with current members
                let old_members = members.read().await.clone();

                let joined: Vec<String> = new_members.difference(&old_members).cloned().collect();
                let left: Vec<String> = old_members.difference(&new_members).cloned().collect();

                if joined.is_empty() && left.is_empty() {
                    continue;
                }

                // Update ring
                {
                    let mut ring = ring.write().await;
                    for nid in &joined {
                        ring.add_node(nid);
                        info!(node_id = %nid, "Node joined cluster");
                    }
                    for nid in &left {
                        ring.remove_node(nid);
                        info!(node_id = %nid, "Node left cluster");
                    }
                }

                // Update members and grpc_addrs
                *members.write().await = new_members.clone();
                *grpc_addrs.write().await = new_grpc_addrs.clone();

                // Update metrics
                valka_core::metrics::set_cluster_members(new_members.len() as f64);

                // Emit events
                for nid in &joined {
                    let addr = grpc_addrs
                        .read()
                        .await
                        .get(nid)
                        .cloned()
                        .unwrap_or_default();
                    let _ = event_tx.send(ClusterEvent::NodeJoined {
                        node_id: NodeId(nid.clone()),
                        grpc_addr: addr,
                    });
                }
                for nid in &left {
                    let _ = event_tx.send(ClusterEvent::NodeLeft {
                        node_id: NodeId(nid.clone()),
                    });
                }

                if !joined.is_empty() || !left.is_empty() {
                    let _ = event_tx.send(ClusterEvent::PartitionsRebalanced);
                }
            }
        });
    }

    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    pub fn num_partitions(&self) -> i32 {
        self.num_partitions
    }

    /// Check if this node owns the given partition
    pub async fn owns_partition(&self, queue_name: &str, partition_id: i32) -> bool {
        let ring = self.ring.read().await;
        let key = format!("{queue_name}:{partition_id}");
        ring.get_node(&key)
            .map(|n| n == self.node_id.0)
            .unwrap_or(true) // Single-node: always own
    }

    /// Returns None if we own the partition, else the owner's gRPC addr
    pub async fn get_partition_owner_addr(
        &self,
        queue_name: &str,
        partition_id: i32,
    ) -> Option<String> {
        let ring = self.ring.read().await;
        let key = format!("{queue_name}:{partition_id}");
        let owner = ring.get_node(&key)?;
        if owner == self.node_id.0 {
            return None;
        }
        drop(ring);
        self.grpc_addrs.read().await.get(&owner).cloned()
    }

    /// Look up a node's gRPC address
    pub async fn get_node_grpc_addr(&self, node_id: &str) -> Option<String> {
        self.grpc_addrs.read().await.get(node_id).cloned()
    }

    /// Subscribe to cluster events
    pub fn subscribe_events(&self) -> broadcast::Receiver<ClusterEvent> {
        self.event_tx.subscribe()
    }

    /// Get all members
    pub async fn members(&self) -> HashSet<String> {
        self.members.read().await.clone()
    }

    /// Whether this manager is in clustered mode
    pub fn is_clustered(&self) -> bool {
        self.chitchat_handle.is_some()
    }

    /// Shutdown the gossip layer
    pub async fn shutdown(self) {
        if let Some(handle) = self.chitchat_handle {
            if let Err(e) = handle.shutdown().await {
                warn!(error = %e, "Error shutting down chitchat");
            }
            info!("Cluster gossip stopped");
        }
    }
}
