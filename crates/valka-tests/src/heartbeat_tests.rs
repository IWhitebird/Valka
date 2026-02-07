use chrono::{Duration, Utc};
use tokio::sync::mpsc;
use valka_core::WorkerId;
use valka_dispatcher::heartbeat::{WorkerStatus, check_heartbeat};
use valka_dispatcher::worker_handle::WorkerHandle;
use valka_proto::WorkerResponse;

fn make_handle(concurrency: i32) -> WorkerHandle {
    let (tx, _rx) = mpsc::channel::<WorkerResponse>(16);
    WorkerHandle::new(
        WorkerId::new(),
        "test-worker".to_string(),
        vec!["default".to_string()],
        concurrency,
        tx,
        String::new(),
    )
}

#[test]
fn test_heartbeat_alive() {
    let handle = make_handle(1);
    // Just created, should be alive
    assert_eq!(check_heartbeat(&handle), WorkerStatus::Alive);
}

#[test]
fn test_heartbeat_suspect() {
    let mut handle = make_handle(1);
    // Set heartbeat to 15 seconds ago
    handle.last_heartbeat = Utc::now() - Duration::seconds(15);
    assert_eq!(check_heartbeat(&handle), WorkerStatus::Suspect);
}

#[test]
fn test_heartbeat_dead() {
    let mut handle = make_handle(1);
    // Set heartbeat to 60 seconds ago
    handle.last_heartbeat = Utc::now() - Duration::seconds(60);
    assert_eq!(check_heartbeat(&handle), WorkerStatus::Dead);
}

#[test]
fn test_heartbeat_boundary_alive_suspect() {
    let mut handle = make_handle(1);
    // Just under the 10s threshold — should be alive
    handle.last_heartbeat = Utc::now() - Duration::seconds(9);
    assert_eq!(check_heartbeat(&handle), WorkerStatus::Alive);

    // Just over — should be suspect
    handle.last_heartbeat = Utc::now() - Duration::seconds(11);
    assert_eq!(check_heartbeat(&handle), WorkerStatus::Suspect);
}

#[test]
fn test_heartbeat_boundary_suspect_dead() {
    let mut handle = make_handle(1);
    // Just under the 30s threshold — should be suspect
    handle.last_heartbeat = Utc::now() - Duration::seconds(29);
    assert_eq!(check_heartbeat(&handle), WorkerStatus::Suspect);

    // Just over — should be dead
    handle.last_heartbeat = Utc::now() - Duration::seconds(31);
    assert_eq!(check_heartbeat(&handle), WorkerStatus::Dead);
}

#[test]
fn test_heartbeat_just_created() {
    let handle = make_handle(5);
    // Fresh worker should always be alive
    let status = check_heartbeat(&handle);
    assert_eq!(status, WorkerStatus::Alive);
}

#[test]
fn test_heartbeat_after_update() {
    let mut handle = make_handle(1);
    // Set to dead timing
    handle.last_heartbeat = Utc::now() - Duration::seconds(60);
    assert_eq!(check_heartbeat(&handle), WorkerStatus::Dead);

    // Update heartbeat
    handle.update_heartbeat();
    assert_eq!(check_heartbeat(&handle), WorkerStatus::Alive);
}

#[tokio::test]
async fn test_heartbeat_checker_detects_dead_worker() {
    use dashmap::DashMap;
    use std::sync::Arc;
    use tokio::sync::watch;

    let workers = Arc::new(DashMap::new());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (dead_tx, mut dead_rx) = mpsc::channel::<WorkerId>(16);

    // Create a worker with old heartbeat (dead)
    let worker_id = WorkerId::new();
    let mut handle = make_handle(1);
    handle.worker_id = worker_id.clone();
    handle.last_heartbeat = Utc::now() - Duration::seconds(60);
    workers.insert(worker_id.0.clone(), handle);

    // Start heartbeat checker
    let workers_clone = workers.clone();
    let checker = tokio::spawn(valka_dispatcher::heartbeat::heartbeat_checker(
        workers_clone,
        shutdown_rx,
        dead_tx,
    ));

    // Wait for the checker to detect and report the dead worker
    let dead_id = tokio::time::timeout(tokio::time::Duration::from_secs(10), dead_rx.recv())
        .await
        .expect("Should detect dead worker within timeout")
        .expect("Should receive dead worker ID");

    assert_eq!(dead_id, worker_id);

    // Verify worker was removed from map
    assert!(workers.is_empty(), "Dead worker should be removed from map");

    // Shutdown
    let _ = shutdown_tx.send(true);
    let _ = checker.await;
}
