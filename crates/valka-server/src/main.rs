#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::Result;
use tracing::{error, info};

mod shutdown;

use valka_server::grpc;
use valka_server::rest;
use valka_server::server::{ExitReason, Node, wait_for_exit};

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
        // A stable identity lets a restarted node reclaim its own shards right away. The
        // host name is stable on a VM, a dev machine, a container across restarts and a
        // StatefulSet pod; set VALKA_NODE_ID to choose one explicitly.
        config.node_id = default_node_id();
    }
    info!(node_id = %config.node_id, backend = %config.storage.backend, "Node ID assigned");

    let store = valka_wal::Store::from_config(&config.storage)?;
    let node = Node::build(&config, store).await?;

    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("Failed to install Prometheus recorder");

    let mut node = node;
    let internal_handle = node.internal_listener.take().map(|listener| {
        let (api, local_events, token, shutdown, failed) = (
            node.api.clone(),
            node.event_tx.clone(),
            config.cluster.token.clone(),
            node.shutdown_rx.clone(),
            node.shutdown_tx.clone(),
        );
        let tls = valka_cluster::security::server_tls(&config.cluster);
        tokio::spawn(async move {
            let r = match tls {
                Ok(tls) => {
                    valka_server::internal_grpc::serve_internal(
                        listener,
                        api,
                        local_events,
                        token,
                        tls,
                        shutdown,
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            if let Err(e) = r {
                tracing::error!(error = %e, "internal listener failed");
                let _ = failed.send(true);
            }
        })
    });

    let api_keys = valka_server::auth::ApiKeys::parse(config.api_keys.as_deref());
    if api_keys.is_open() {
        tracing::warn!(
            "the public API is OPEN: anyone who can reach the REST and gRPC ports can create, \
             cancel and delete tasks. Set VALKA_API_KEYS to require API keys."
        );
    }

    let grpc_addr = config.grpc_addr.parse()?;
    let shutdown_tx_grpc = node.shutdown_tx.clone();
    let grpc_handle = tokio::spawn({
        let (api, event_tx, logs, shutdown, keys) = (
            node.api.clone(),
            node.cluster_events.clone(),
            node.logs.clone(),
            node.shutdown_rx.clone(),
            api_keys.clone(),
        );
        async move {
            if let Err(e) = grpc::serve_grpc(grpc_addr, api, event_tx, logs, keys, shutdown).await {
                tracing::error!(error = %e, "gRPC server failed");
                let _ = shutdown_tx_grpc.send(true);
            }
        }
    });

    let http_addr = config.http_addr.parse()?;
    let shutdown_tx_rest = node.shutdown_tx.clone();
    let http_handle = tokio::spawn({
        let (api, event_tx, logs, shutdown, web_dir, admin_token) = (
            node.api.clone(),
            node.cluster_events.clone(),
            node.logs.clone(),
            node.shutdown_rx.clone(),
            config.web_dir.clone(),
            config.admin_token.clone(),
        );
        let node_info = valka_server::cluster::NodeInfo::new(
            &config.grpc_addr,
            &config.http_addr,
            config.wal.flush_interval_ms,
        );
        async move {
            if let Err(e) = rest::serve_rest(
                http_addr,
                api,
                event_tx,
                logs,
                metrics_handle,
                node_info,
                admin_token,
                api_keys,
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

    let reason = wait_for_exit(
        shutdown::wait_for_shutdown(),
        node.engine.poison_watch(),
        node.shutdown_rx.clone(),
        node.engine.drained_watch(),
    )
    .await;
    match &reason {
        ExitReason::Signal => info!("Shutdown signal received, draining..."),
        ExitReason::Poisoned(why) => error!(
            reason = %why,
            "WAL writer poisoned; exiting so the node restarts from the bucket"
        ),
        ExitReason::Failed => error!("a server task failed; exiting"),
        ExitReason::Drained => info!("drained: every shard handed over, exiting"),
    }
    let _ = node.shutdown_tx.send(true);

    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let _ = grpc_handle.await;
        let _ = http_handle.await;
        if let Some(h) = internal_handle {
            let _ = h.await;
        }
    })
    .await;

    // A clean stop hands shards to live peers, or snapshots them for the next start.
    // A poisoned node must not write anything more: it exits and its successor recovers.
    if reason == ExitReason::Signal
        && let Err(e) = node.engine.stop().await
    {
        tracing::error!(error = %e, "engine shutdown failed");
    }

    if reason.is_clean() {
        info!("Valka server stopped");
        Ok(())
    } else {
        anyhow::bail!("Valka server stopped: {reason:?}")
    }
}

/// The host name, reduced to the characters node ids allow.
fn default_node_id() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|h| h.trim().to_string())
        })
        .unwrap_or_default();
    let id: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    if id.is_empty() {
        "valka-1".to_string()
    } else {
        id
    }
}
