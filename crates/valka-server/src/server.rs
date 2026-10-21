//! Server assembly helpers: build every service from a config, for `main` and tests.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch};

use valka_cluster::PeerClients;

use crate::api::Api;
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
    /// Events of the shards this node serves.
    pub event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    /// The whole cluster's events (this node's plus its peers', while anyone listens).
    pub cluster_events: broadcast::Sender<valka_proto::TaskEvent>,
    /// The request layer (routing to shard owners, scatter-gather).
    pub api: Api,
    /// The internal listener, bound when `cluster.enabled` (or by tests).
    pub internal_listener: Option<tokio::net::TcpListener>,
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

        let c = &config.cluster;
        let engine_cfg = EngineConfig {
            node_id: node_id.0.clone(),
            wal: config.wal.clone(),
            scheduler: config.scheduler.clone(),
            cluster: c.clone(),
            advert: valka_wal::lease::NodeAdvert {
                grpc_addr: Some(
                    c.advertise_grpc_addr
                        .clone()
                        .unwrap_or_else(|| config.grpc_addr.clone()),
                ),
                http_addr: Some(
                    c.advertise_http_addr
                        .clone()
                        .unwrap_or_else(|| config.http_addr.clone()),
                ),
                internal_addr: c.enabled.then(|| {
                    c.advertise_internal_addr
                        .clone()
                        .unwrap_or_else(|| c.internal_addr.clone())
                }),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            feeder_interval: Duration::from_millis(config.matching.feeder_interval_ms.max(1)),
            feeder_batch_size: config.matching.feeder_batch_size.max(1),
            limits: config.limits.clone(),
        };
        let engine = Engine::open_with(
            store.clone(),
            engine_cfg,
            valka_engine::TokioClock::new(),
            Arc::new(MatchingSink::new(matching.clone())),
        )
        .await?;
        crate::convert::spawn_event_bridge(&engine, event_tx.clone());

        let (log_tx, log_rx) = mpsc::channel(10_000);
        let logs = LogIngester::new(
            store.clone(),
            config.log_ingester.batch_size,
            Duration::from_millis(config.log_ingester.flush_interval_ms.max(1)),
        );
        tokio::spawn(logs.clone().run(log_rx, shutdown_rx.clone()));

        let dispatcher =
            DispatcherService::new(matching.clone(), engine.clone(), node_id.clone(), log_tx);
        let peers = PeerClients::new(
            &node_id.0,
            config.cluster.token.clone(),
            valka_cluster::security::client_tls(&config.cluster)?,
            Duration::from_secs(10),
        );
        let api = Api::new(engine.clone(), dispatcher.clone(), peers);
        // Workers here may run tasks owned anywhere: route their side through the Api.
        dispatcher.set_router(std::sync::Arc::new(api.clone()));
        crate::feeder::spawn_remote_feeder(api.clone(), shutdown_rx.clone());
        let (cluster_events, _) = broadcast::channel::<valka_proto::TaskEvent>(4096);
        crate::relay::spawn_event_relay(
            api.clone(),
            event_tx.clone(),
            cluster_events.clone(),
            shutdown_rx.clone(),
        );
        // Bind before serving anything so a bad address fails the start.
        let internal_listener = if config.cluster.enabled {
            Some(tokio::net::TcpListener::bind(&config.cluster.internal_addr).await?)
        } else {
            None
        };

        let (_hb_handle, mut dead_rx) = dispatcher.start_heartbeat_checker(shutdown_rx.clone());
        let d2 = dispatcher.clone();
        tokio::spawn(async move {
            // The checker closed the connection; deregister it now (its stream handler may
            // be busy), resetting its delivered signals. Only that connection: the worker
            // may already have reconnected.
            while let Some(dead) = dead_rx.recv().await {
                d2.deregister_connection(&dead.worker_id, dead.conn_id)
                    .await;
            }
        });

        Ok(Node {
            node_id,
            engine,
            matching,
            dispatcher,
            logs,
            event_tx,
            cluster_events,
            api,
            internal_listener,
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
    /// The node was drained (admin API): it handed its shards over and exits.
    Drained,
}

impl ExitReason {
    pub fn is_clean(&self) -> bool {
        matches!(self, ExitReason::Signal | ExitReason::Drained)
    }
}

/// Resolve as soon as any stop condition occurs. A closed channel only disables its branch.
pub async fn wait_for_exit(
    signal: impl std::future::Future<Output = ()>,
    mut poison: watch::Receiver<Option<String>>,
    mut shutdown: watch::Receiver<bool>,
    mut drained: watch::Receiver<bool>,
) -> ExitReason {
    let poisoned = async move {
        poison
            .wait_for(|p| p.is_some())
            .await
            .map(|p| p.clone().unwrap_or_default())
    };
    let failed = async move { shutdown.wait_for(|s| *s).await.map(|_| ()) };
    let drained = async move { drained.wait_for(|d| *d).await.map(|_| ()) };
    tokio::select! {
        _ = signal => ExitReason::Signal,
        Ok(reason) = poisoned => ExitReason::Poisoned(reason),
        Ok(()) = failed => ExitReason::Failed,
        Ok(()) = drained => ExitReason::Drained,
    }
}
