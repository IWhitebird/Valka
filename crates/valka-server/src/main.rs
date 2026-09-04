#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::sync::Arc;

use anyhow::Result;
use tracing::info;

mod shutdown;

use valka_server::grpc;
use valka_server::rest;
use valka_server::server::Node;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "valka=info,tower_http=info".into()),
        )
        .init();

    info!("Starting Valka server");

    let config_path = std::env::args().nth(1);
    let mut config = valka_core::ServerConfig::load(config_path.as_deref())?;

    if config.node_id.is_empty() {
        // Diskless nodes get a fresh identity each start. Set VALKA_NODE_ID for a
        // stable identity (recommended in production so the node replays its own WAL).
        config.node_id = uuid::Uuid::now_v7().to_string();
    }
    info!(node_id = %config.node_id, backend = %config.storage.backend, "Node ID assigned");

    let store = valka_wal::Store::from_config(&config.storage)?;
    let node = Node::build(&config, store).await?;

    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("Failed to install Prometheus recorder");

    // Event relay across nodes (clustered mode only).
    if node.cluster.is_clustered() {
        let relay_cluster = node.cluster.clone();
        let relay_forwarder = node.forwarder.clone();
        let relay_event_rx = node.event_tx.subscribe();
        let relay_shutdown = node.shutdown_rx.clone();
        tokio::spawn(async move {
            valka_cluster::event_relay::run_event_relay(
                relay_cluster,
                relay_forwarder,
                relay_event_rx,
                relay_shutdown,
            )
            .await;
        });
    }

    let grpc_addr = config.grpc_addr.parse()?;
    let shutdown_tx_grpc = node.shutdown_tx.clone();
    let grpc_handle = tokio::spawn({
        let (engine, dispatcher, event_tx, node_id, logs, shutdown) = (
            node.engine.clone(),
            node.dispatcher.clone(),
            node.event_tx.clone(),
            node.node_id.clone(),
            node.logs.clone(),
            node.shutdown_rx.clone(),
        );
        async move {
            if let Err(e) = grpc::serve_grpc(
                grpc_addr, engine, dispatcher, event_tx, node_id, logs, shutdown,
            )
            .await
            {
                tracing::error!(error = %e, "gRPC server failed");
                let _ = shutdown_tx_grpc.send(true);
            }
        }
    });

    let http_addr = config.http_addr.parse()?;
    let shutdown_tx_rest = node.shutdown_tx.clone();
    let http_handle = tokio::spawn({
        let (engine, dispatcher, event_tx, cluster, logs, shutdown, web_dir) = (
            node.engine.clone(),
            node.dispatcher.clone(),
            node.event_tx.clone(),
            node.cluster.clone(),
            node.logs.clone(),
            node.shutdown_rx.clone(),
            config.web_dir.clone(),
        );
        let node_info = valka_server::cluster::NodeInfo::new(
            &config.grpc_addr,
            &config.http_addr,
            config.wal.flush_interval_ms,
        );
        async move {
            if let Err(e) = rest::serve_rest(
                http_addr,
                engine,
                event_tx,
                dispatcher,
                logs,
                metrics_handle,
                cluster,
                node_info,
                web_dir,
                shutdown,
            )
            .await
            {
                tracing::error!(error = %e, "REST server failed");
                let _ = shutdown_tx_rest.send(true);
            }
        }
    });

    info!(
        grpc_addr = %config.grpc_addr,
        http_addr = %config.http_addr,
        "Valka server started"
    );

    shutdown::wait_for_shutdown().await;
    info!("Shutdown signal received, draining...");
    let _ = node.shutdown_tx.send(true);

    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let _ = grpc_handle.await;
        let _ = http_handle.await;
    })
    .await;

    // Flush the WAL and snapshot dirty shards so the next start replays little.
    if let Err(e) = node.engine.shutdown().await {
        tracing::error!(error = %e, "engine shutdown failed");
    }

    if let Ok(cluster) = Arc::try_unwrap(node.cluster) {
        cluster.shutdown().await;
    }

    info!("Valka server stopped");
    Ok(())
}
