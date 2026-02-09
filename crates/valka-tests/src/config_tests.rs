use valka_core::{GossipConfig, LogIngesterConfig, MatchingConfig, SchedulerConfig, ServerConfig};

#[test]
fn test_matching_config_defaults() {
    let config = MatchingConfig::default();
    assert_eq!(config.num_partitions, 4);
    assert_eq!(config.branching_factor, 3);
    assert_eq!(config.max_buffer_per_partition, 1000);
    assert_eq!(config.task_reader_batch_size, 50);
    assert_eq!(config.task_reader_poll_busy_ms, 10);
    assert_eq!(config.task_reader_poll_idle_ms, 200);
}

#[test]
fn test_scheduler_config_defaults() {
    let config = SchedulerConfig::default();
    assert_eq!(config.reaper_interval_secs, 10);
    assert_eq!(config.lease_timeout_secs, 60);
    assert_eq!(config.retry_base_delay_secs, 1);
    assert_eq!(config.retry_max_delay_secs, 3600);
    assert_eq!(config.dlq_check_interval_secs, 30);
    assert_eq!(config.delayed_check_interval_secs, 5);
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
    assert!(!config.database_url.is_empty());
    // Verify sub-configs are nested correctly
    assert_eq!(config.matching.num_partitions, 4);
    assert_eq!(config.scheduler.reaper_interval_secs, 10);
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
        task_reader_batch_size: 100,
        task_reader_poll_busy_ms: 5,
        task_reader_poll_idle_ms: 100,
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
