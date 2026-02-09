use valka_core::{
    MatchingConfig, NodeId, TaskId, TaskRunId, TaskStatus, WorkerId, partition_for_task,
};

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
    assert_eq!(config.grpc_addr, "0.0.0.0:50051");
    assert_eq!(config.http_addr, "0.0.0.0:8989");
    assert!(!config.database_url.is_empty());
    assert_eq!(config.scheduler.reaper_interval_secs, 10);
    assert_eq!(config.scheduler.lease_timeout_secs, 60);
    assert_eq!(config.log_ingester.batch_size, 100);
    assert_eq!(config.log_ingester.flush_interval_ms, 500);
}

#[test]
fn test_uuid_v7_time_sortable() {
    let ids: Vec<TaskId> = (0..100).map(|_| TaskId::new()).collect();
    for window in ids.windows(2) {
        assert!(
            window[0].0 <= window[1].0,
            "UUIDv7 IDs should be lexicographically sortable in creation order"
        );
    }
}

#[test]
fn test_all_id_types_unique() {
    let task_ids: Vec<String> = (0..10).map(|_| TaskId::new().0).collect();
    let worker_ids: Vec<String> = (0..10).map(|_| WorkerId::new().0).collect();
    let run_ids: Vec<String> = (0..10).map(|_| TaskRunId::new().0).collect();
    let node_ids: Vec<String> = (0..10).map(|_| NodeId::new().0).collect();

    let mut all: Vec<&str> = Vec::new();
    all.extend(task_ids.iter().map(|s| s.as_str()));
    all.extend(worker_ids.iter().map(|s| s.as_str()));
    all.extend(run_ids.iter().map(|s| s.as_str()));
    all.extend(node_ids.iter().map(|s| s.as_str()));

    let unique: std::collections::HashSet<&str> = all.iter().copied().collect();
    assert_eq!(
        unique.len(),
        40,
        "All 40 IDs should be unique, got {}",
        unique.len()
    );
}

#[test]
fn test_partition_for_task_single_partition() {
    for i in 0..50 {
        let p = partition_for_task("queue", &format!("task-{i}"), 1);
        assert_eq!(
            p.0, 0,
            "With 1 partition, all tasks should map to partition 0"
        );
    }
}

#[test]
fn test_partition_different_queues_different_partitions() {
    // Different queue names should generally produce different partitions for same task_id
    let mut results = std::collections::HashSet::new();
    for q in [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ] {
        let p = partition_for_task(q, "same-task-id", 16);
        results.insert(p.0);
    }
    assert!(
        results.len() > 1,
        "Different queues should produce at least some different partitions"
    );
}

#[test]
fn test_task_status_invalid_string() {
    assert!(TaskStatus::from_str_status("BOGUS").is_none());
    assert!(TaskStatus::from_str_status("").is_none());
    assert!(TaskStatus::from_str_status("UNKNOWN").is_none());
}

#[test]
fn test_task_status_case_sensitive() {
    // All lowercase should fail
    assert!(TaskStatus::from_str_status("pending").is_none());
    assert!(TaskStatus::from_str_status("completed").is_none());
    assert!(TaskStatus::from_str_status("Pending").is_none());
}

#[test]
fn test_task_status_display() {
    assert_eq!(format!("{}", TaskStatus::Pending), "PENDING");
    assert_eq!(format!("{}", TaskStatus::DeadLetter), "DEAD_LETTER");
}

#[test]
fn test_id_display_and_as_ref() {
    let task_id = TaskId::new();
    let display = format!("{task_id}");
    let as_ref: &str = task_id.as_ref();
    assert_eq!(display, as_ref);
    assert_eq!(display, task_id.0);
}
