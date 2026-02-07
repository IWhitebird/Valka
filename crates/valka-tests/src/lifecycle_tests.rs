use valka_core::{MatchingConfig, TaskId, TaskStatus, partition_for_task};

#[test]
fn test_task_id_generation() {
    let id1 = TaskId::new();
    let id2 = TaskId::new();
    assert_ne!(id1, id2, "Generated IDs should be unique");
    assert!(!id1.0.is_empty(), "ID should not be empty");
}

#[test]
fn test_partition_assignment_deterministic() {
    let partition = partition_for_task("email.send", "task-123", 4);
    assert!(
        partition.0 >= 0 && partition.0 < 4,
        "Partition should be in range"
    );

    let partition2 = partition_for_task("email.send", "task-123", 4);
    assert_eq!(
        partition.0, partition2.0,
        "Same inputs should produce same partition"
    );
}

#[test]
fn test_partition_distribution() {
    // Check that partitions are distributed across the range
    let mut seen = std::collections::HashSet::new();
    for i in 0..100 {
        let p = partition_for_task("test", &format!("task-{i}"), 4);
        seen.insert(p.0);
    }
    assert!(
        seen.len() > 1,
        "Partitions should be distributed (got {} unique)",
        seen.len()
    );
}

#[test]
fn test_task_status_roundtrip() {
    let statuses = [
        TaskStatus::Pending,
        TaskStatus::Dispatching,
        TaskStatus::Running,
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Retry,
        TaskStatus::DeadLetter,
        TaskStatus::Cancelled,
    ];

    for status in &statuses {
        let s = status.as_str();
        let parsed = TaskStatus::from_str_status(s).expect("Should parse status");
        assert_eq!(*status, parsed);
    }
}

#[test]
fn test_config_defaults() {
    let config = MatchingConfig::default();
    assert_eq!(config.num_partitions, 4);
    assert_eq!(config.branching_factor, 3);
    assert_eq!(config.max_buffer_per_partition, 1000);
    assert_eq!(config.task_reader_batch_size, 50);
}

#[test]
fn test_server_config_defaults() {
    let config = valka_core::ServerConfig::default();
    assert_eq!(config.grpc_addr, "[::1]:50051");
    assert_eq!(config.http_addr, "0.0.0.0:8080");
    assert!(!config.database_url.is_empty());
    assert_eq!(config.scheduler.reaper_interval_secs, 10);
    assert_eq!(config.scheduler.lease_timeout_secs, 60);
    assert_eq!(config.log_ingester.batch_size, 100);
    assert_eq!(config.log_ingester.flush_interval_ms, 500);
}
