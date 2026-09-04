use valka_core::{GossipConfig, LogIngesterConfig, MatchingConfig, SchedulerConfig, ServerConfig};

#[test]
fn test_matching_config_defaults() {
    let config = MatchingConfig::default();
    assert_eq!(config.num_partitions, 4);
    assert_eq!(config.branching_factor, 3);
    assert_eq!(config.max_buffer_per_partition, 1000);
    assert_eq!(config.feeder_interval_ms, 20);
    assert_eq!(config.feeder_batch_size, 200);
}

#[test]
fn test_scheduler_config_defaults() {
    let config = SchedulerConfig::default();
    assert_eq!(config.timer_tick_ms, 100);
    assert_eq!(config.lease_grace_secs, 30);
    assert_eq!(config.recovery_grace_secs, 60);
    assert_eq!(config.heartbeat_lease_secs, 60);
    assert_eq!(config.retry_base_delay_secs, 1);
    assert_eq!(config.retry_max_delay_secs, 3600);
}

#[test]
fn test_storage_and_wal_config_defaults() {
    let storage = valka_core::StorageConfig::default();
    assert_eq!(storage.backend, "local");
    assert_eq!(storage.path, "./data");
    assert!(!storage.allow_http);
    let wal = valka_core::WalConfig::default();
    assert_eq!(wal.flush_interval_ms, 50);
    assert_eq!(wal.max_batch_bytes, 4 * 1024 * 1024);
    assert_eq!(wal.snapshot_interval_secs, 60);
    assert_eq!(wal.snapshots_to_keep, 2);
    assert_eq!(wal.completed_retention_secs, 24 * 3600);
}

#[test]
fn test_log_ingester_config_defaults() {
    let config = LogIngesterConfig::default();
    assert_eq!(config.batch_size, 100);
    assert_eq!(config.flush_interval_ms, 500);
}

#[test]
fn test_gossip_config_defaults() {
    let config = GossipConfig::default();
    assert_eq!(config.listen_addr, "0.0.0.0:7280");
    assert!(config.seed_nodes.is_empty());
    assert_eq!(config.cluster_id, "valka");
}

#[test]
fn test_server_config_all_sub_configs() {
    let config = ServerConfig::default();
    assert_eq!(config.grpc_addr, "0.0.0.0:50051");
    assert_eq!(config.http_addr, "0.0.0.0:8989");
    assert_eq!(config.storage.backend, "local");
    // Verify sub-configs are nested correctly
    assert_eq!(config.matching.num_partitions, 4);
    assert_eq!(config.scheduler.timer_tick_ms, 100);
    assert_eq!(config.log_ingester.batch_size, 100);
    assert_eq!(config.gossip.cluster_id, "valka");
}

#[test]
fn test_config_load_missing_file() {
    // Loading with a nonexistent file should still work (falls back to defaults + env)
    let result = ServerConfig::load(Some("/nonexistent/path/valka.toml"));
    assert!(result.is_ok(), "Should not fail with missing config file");
    let config = result.unwrap();
    assert_eq!(config.matching.num_partitions, 4);
}

#[test]
fn test_matching_config_custom_values() {
    let config = MatchingConfig {
        num_partitions: 16,
        branching_factor: 4,
        max_buffer_per_partition: 500,
        feeder_interval_ms: 5,
        feeder_batch_size: 100,
    };
    assert_eq!(config.num_partitions, 16);
    assert_eq!(config.branching_factor, 4);
    assert_eq!(config.max_buffer_per_partition, 500);
}

#[test]
fn test_web_dir_default() {
    let config = ServerConfig::default();
    assert_eq!(config.web_dir, "web/dist");
}
