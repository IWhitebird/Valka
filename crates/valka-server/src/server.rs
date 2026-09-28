//! Server assembly helpers: build every service from a config, for `main` and tests.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::info;

use valka_cluster::{ClusterManager, NodeForwarder};
use valka_core::{NodeId, ServerConfig};
use valka_dispatcher::DispatcherService;
use valka_engine::{Engine, EngineConfig, LogIngester};
use valka_matching::{MatchingService, MatchingSink};
use valka_wal::Store;

/// Everything a node runs, wired together.
pub struct Node {
    pub node_id: NodeId,
    pub engine: Engine,
    pub matching: MatchingService,
    pub dispatcher: DispatcherService,
    pub logs: Arc<LogIngester>,
    pub event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    pub cluster: Arc<ClusterManager>,
    pub forwarder: NodeForwarder,
    pub shutdown_tx: watch::Sender<bool>,
    pub shutdown_rx: watch::Receiver<bool>,
}

impl Node {
    /// Open the store, recover the engine, and wire matching/dispatcher/logs/events.
    pub async fn build(config: &ServerConfig, store: Store) -> anyhow::Result<Node> {
        let node_id = NodeId(config.node_id.clone());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (event_tx, _) = broadcast::channel::<valka_proto::TaskEvent>(4096);

        let matching = MatchingService::new(config.matching.clone());

        let engine_cfg = EngineConfig {
            node_id: node_id.0.clone(),
            wal: config.wal.clone(),
            scheduler: config.scheduler.clone(),
            feeder_interval: Duration::from_millis(config.matching.feeder_interval_ms.max(1)),
            feeder_batch_size: config.matching.feeder_batch_size.max(1),
            // The memory backend is a single process by definition.
            trust_self: config.storage.backend == "memory",
        };
        let engine = Engine::open_with(
            store.clone(),
            engine_cfg,
            valka_engine::TokioClock::new(),
            Arc::new(MatchingSink::new(matching.clone())),
        )
        .await?;
        crate::convert::spawn_event_bridge(&engine, event_tx.clone());

        let cluster = Arc::new(if config.gossip.seed_nodes.is_empty() {
            info!("Starting in single-node mode (no seed_nodes configured)");
            ClusterManager::new_single_node(node_id.clone(), config.matching.num_partitions)
        } else {
            info!(seeds = ?config.gossip.seed_nodes, "Starting in clustered mode");
            ClusterManager::new_clustered(
                node_id.clone(),
                config.matching.num_partitions,
                &config.gossip,
                &config.grpc_addr,
            )
            .await?
        });
        let forwarder = NodeForwarder::new();

        let (log_tx, log_rx) = mpsc::channel(10_000);
        let logs = LogIngester::new(
            store.clone(),
            config.log_ingester.batch_size,
            Duration::from_millis(config.log_ingester.flush_interval_ms.max(1)),
        );
        tokio::spawn(logs.clone().run(log_rx, shutdown_rx.clone()));

        let dispatcher =
            DispatcherService::new(matching.clone(), engine.clone(), node_id.clone(), log_tx);

        let (_hb_handle, mut dead_rx) = dispatcher.start_heartbeat_checker(shutdown_rx.clone());
        let d2 = dispatcher.clone();
        tokio::spawn(async move {
            while let Some(worker_id) = dead_rx.recv().await {
                d2.deregister_worker(&worker_id).await;
            }
        });

        Ok(Node {
            node_id,
            engine,
            matching,
            dispatcher,
            logs,
            event_tx,
            cluster,
            forwarder,
            shutdown_tx,
            shutdown_rx,
        })
    }
}

/// Why the node is stopping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    /// SIGTERM or Ctrl-C: drain and exit cleanly.
    Signal,
    /// The WAL writer can no longer commit; RAM may be ahead of the bucket, so the node
    /// must restart and rebuild from it.
    Poisoned(String),
    /// A server task failed and requested shutdown.
    Failed,
}

impl ExitReason {
    pub fn is_clean(&self) -> bool {
        matches!(self, ExitReason::Signal)
    }
}

/// Resolve as soon as any stop condition occurs. A closed channel only disables its branch.
pub async fn wait_for_exit(
    signal: impl std::future::Future<Output = ()>,
    mut poison: watch::Receiver<Option<String>>,
    mut shutdown: watch::Receiver<bool>,
) -> ExitReason {
    let poisoned = async move {
        poison
            .wait_for(|p| p.is_some())
            .await
            .map(|p| p.clone().unwrap_or_default())
    };
    let failed = async move { shutdown.wait_for(|s| *s).await.map(|_| ()) };
    tokio::select! {
        _ = signal => ExitReason::Signal,
        Ok(reason) = poisoned => ExitReason::Poisoned(reason),
        Ok(()) = failed => ExitReason::Failed,
    }
}
