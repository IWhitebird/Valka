#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::Result;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::info;

mod grpc;
mod rest;
mod server;
mod shutdown;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "valka=info,tower_http=info".into()),
        )
        .init();

    info!("Starting Valka server");

    // Load configuration
    let config_path = std::env::args().nth(1);
    let mut config = valka_core::ServerConfig::load(config_path.as_deref())?;

    if config.node_id.is_empty() {
        config.node_id = uuid::Uuid::now_v7().to_string();
    }
    let node_id = valka_core::NodeId(config.node_id.clone());

    info!(node_id = %node_id, "Node ID assigned");

    // Create database pool
    let pool = valka_db::pool::create_pool(&config.database_url).await?;

    // Run migrations
    valka_db::migrations::run_migrations(&pool).await?;

    // Recover orphaned DISPATCHING tasks (crash recovery)
    let recovered =
        valka_db::queries::tasks::recover_orphaned_dispatching(&pool).await?;
    if !recovered.is_empty() {
        info!(count = recovered.len(), "Recovered orphaned DISPATCHING tasks to PENDING");
    }

    // Shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Event broadcast channel
    let (event_tx, _) = broadcast::channel::<valka_proto::TaskEvent>(4096);

    // Log ingestion channel
    let (log_tx, log_rx) = mpsc::channel::<valka_proto::LogEntry>(10000);

    // Initialize services
    let matching = valka_matching::MatchingService::new(config.matching.clone());
    let _cluster = valka_cluster::ClusterManager::new_single_node(
        node_id.clone(),
        config.matching.num_partitions,
    );

    let dispatcher = valka_dispatcher::DispatcherService::new(
        matching.clone(),
        pool.clone(),
        node_id.clone(),
        event_tx.clone(),
        log_tx.clone(),
    );

    // Start heartbeat checker
    let (_hb_handle, mut dead_rx) = dispatcher.start_heartbeat_checker(shutdown_rx.clone());

    // Handle dead workers
    let dispatcher_clone = dispatcher.clone();
    tokio::spawn(async move {
        while let Some(worker_id) = dead_rx.recv().await {
            dispatcher_clone.deregister_worker(&worker_id).await;
        }
    });

    // Start scheduler
    let scheduler_pool = pool.clone();
    let scheduler_config = config.scheduler.clone();
    let scheduler_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        server::run_scheduler(scheduler_pool, scheduler_config, scheduler_shutdown).await;
    });

    // Start log ingester
    let log_pool = pool.clone();
    let log_config = config.log_ingester.clone();
    let log_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        server::run_log_ingester(log_pool, log_config, log_rx, log_shutdown).await;
    });

    // Start TaskReaders for all partitions (they'll handle any queue dynamically)
    // For now, we start a task reader discovery loop that spawns readers for known queues
    let tr_pool = pool.clone();
    let tr_matching = matching.clone();
    let tr_config = config.matching.clone();
    let tr_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        server::run_task_reader_manager(tr_pool, tr_matching, tr_config, tr_shutdown).await;
    });

    // Install metrics exporter
    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("Failed to install Prometheus recorder");

    // Start gRPC server
    let grpc_addr = config.grpc_addr.parse()?;
    let grpc_dispatcher = dispatcher.clone();
    let grpc_pool = pool.clone();
    let grpc_event_tx = event_tx.clone();
    let grpc_matching = matching.clone();
    let grpc_node_id = node_id.clone();
    let grpc_shutdown = shutdown_rx.clone();

    let grpc_handle = tokio::spawn(async move {
        grpc::serve_grpc(
            grpc_addr,
            grpc_pool,
            grpc_dispatcher,
            grpc_matching,
            grpc_event_tx,
            grpc_node_id,
            grpc_shutdown,
        )
        .await
    });

    // Start REST/HTTP server
    let http_addr = config.http_addr.parse()?;
    let rest_pool = pool.clone();
    let rest_event_tx = event_tx.clone();
    let rest_matching = matching.clone();
    let rest_dispatcher = dispatcher.clone();
    let rest_shutdown = shutdown_rx.clone();

    let http_handle = tokio::spawn(async move {
        rest::serve_rest(
            http_addr,
            rest_pool,
            rest_event_tx,
            rest_matching,
            rest_dispatcher,
            metrics_handle,
            config.web_dir.clone(),
            rest_shutdown,
        )
        .await
    });

    info!(
        grpc_addr = %config.grpc_addr,
        http_addr = %config.http_addr,
        "Valka server started"
    );

    // Wait for shutdown signal
    shutdown::wait_for_shutdown().await;
    info!("Shutdown signal received, draining...");
    let _ = shutdown_tx.send(true);

    // Wait for tasks to complete
    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(30), async {
        let _ = grpc_handle.await;
        let _ = http_handle.await;
    })
    .await;

    info!("Valka server stopped");
    Ok(())
}
