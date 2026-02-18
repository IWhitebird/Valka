use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;

use valka_cluster::{ClusterManager, NodeForwarder};
use valka_core::{GossipConfig, MatchingConfig, NodeId, TaskId, partition_for_task};
use valka_db::queries::tasks::CreateTaskParams;
use valka_dispatcher::DispatcherService;
use valka_matching::MatchingService;
use valka_proto::*;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

#[allow(dead_code)] // Fields kept alive for the duration of the test.
struct TestNode {
    node_id: NodeId,
    pool: PgPool,
    cluster: Arc<ClusterManager>,
    matching: MatchingService,
    dispatcher: DispatcherService,
    forwarder: NodeForwarder,
    event_tx: broadcast::Sender<TaskEvent>,
    grpc_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    server_handle: JoinHandle<()>,
}

impl TestNode {
    async fn start(
        pool: PgPool,
        node_name: &str,
        gossip_port: u16,
        grpc_port: u16,
        seed_gossip_ports: Vec<u16>,
        cluster_id: &str,
        num_partitions: i32,
    ) -> Self {
        let node_id = NodeId(node_name.to_string());
        let matching = MatchingService::new(MatchingConfig {
            num_partitions,
            ..MatchingConfig::default()
        });
        let (event_tx, _) = broadcast::channel::<TaskEvent>(128);
        let (log_tx, _log_rx) = mpsc::channel::<LogEntry>(128);

        let dispatcher = DispatcherService::new(
            matching.clone(),
            pool.clone(),
            node_id.clone(),
            event_tx.clone(),
            log_tx.clone(),
        );

        let forwarder = NodeForwarder::new();

        let grpc_addr: SocketAddr = format!("127.0.0.1:{grpc_port}").parse().unwrap();
        let gossip_cfg = gossip_config(gossip_port, seed_gossip_ports, cluster_id);

        let cluster = Arc::new(
            ClusterManager::new_clustered(
                node_id.clone(),
                num_partitions,
                &gossip_cfg,
                &grpc_addr.to_string(),
            )
            .await
            .expect("Failed to create ClusterManager"),
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Clone everything the spawned gRPC server needs, keeping originals for TestNode.
        let srv_pool = pool.clone();
        let srv_dispatcher = dispatcher.clone();
        let srv_matching = matching.clone();
        let srv_event_tx = event_tx.clone();
        let srv_node_id = node_id.clone();
        let srv_cluster = cluster.clone();
        let srv_forwarder = forwarder.clone();

        let server_handle = tokio::spawn(async move {
            valka_server::grpc::serve_grpc(
                grpc_addr,
                srv_pool,
                srv_dispatcher,
                srv_matching,
                srv_event_tx,
                srv_node_id,
                srv_cluster,
                srv_forwarder,
                log_tx,
                shutdown_rx,
            )
            .await
            .expect("gRPC server failed");
        });

        // Give the gRPC server time to bind.
        tokio::time::sleep(Duration::from_millis(300)).await;

        TestNode {
            node_id,
            pool,
            cluster,
            matching,
            dispatcher,
            forwarder,
            event_tx,
            grpc_addr,
            shutdown_tx,
            server_handle,
        }
    }

    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server_handle).await;
        if let Ok(cluster) = Arc::try_unwrap(self.cluster) {
            cluster.shutdown().await;
        }
    }
}

fn gossip_config(listen_port: u16, seeds: Vec<u16>, cluster_id: &str) -> GossipConfig {
    GossipConfig {
        listen_addr: format!("127.0.0.1:{listen_port}"),
        seed_nodes: seeds
            .into_iter()
            .map(|p| format!("127.0.0.1:{p}"))
            .collect(),
        cluster_id: cluster_id.to_string(),
        advertise_addr: None,
    }
}

async fn wait_for_members(cluster: &ClusterManager, expected: usize, timeout_secs: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let members = cluster.members().await;
        if members.len() == expected {
            return;
        }
        if tokio::time::Instant::now() > deadline {
            panic!(
                "Timeout waiting for {expected} members, got {} members: {:?}",
                members.len(),
                members
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Determine which partitions a given cluster node owns for the specified queue.
async fn owned_partitions(
    cluster: &ClusterManager,
    queue: &str,
    num_partitions: i32,
) -> Vec<i32> {
    let mut owned = Vec::new();
    for pid in 0..num_partitions {
        if cluster.owns_partition(queue, pid).await {
            owned.push(pid);
        }
    }
    owned
}

/// Generate a task_id that hashes to one of `target_partitions`.
/// Returns (task_id_string, partition_id).
fn find_task_for_partition(
    queue: &str,
    target_partitions: &[i32],
    num_partitions: i32,
) -> (String, i32) {
    for _ in 0..100_000 {
        let id = TaskId::new().0;
        let pid = partition_for_task(queue, &id, num_partitions);
        if target_partitions.contains(&pid.0) {
            return (id, pid.0);
        }
    }
    panic!(
        "Could not find task_id for partitions {target_partitions:?} after 100k attempts"
    );
}

/// Insert a task into PG with a specific task_id and partition_id.
async fn insert_task(pool: &PgPool, task_id: &str, queue: &str, partition_id: i32) {
    valka_db::queries::tasks::create_task(
        pool,
        CreateTaskParams {
            id: task_id.to_string(),
            queue_name: queue.to_string(),
            task_name: "cluster-test-task".to_string(),
            partition_id,
            input: Some(serde_json::json!({"test": true})),
            priority: 0,
            max_retries: 3,
            timeout_seconds: 300,
            idempotency_key: None,
            metadata: serde_json::json!({}),
            scheduled_at: None,
        },
    )
    .await
    .expect("insert_task failed");
}

/// Connect a mock worker via gRPC bidi stream. Returns (request_sender, response_stream, worker_id).
async fn connect_mock_worker(
    grpc_addr: &SocketAddr,
    queues: &[&str],
    concurrency: i32,
) -> (
    mpsc::Sender<WorkerRequest>,
    tonic::Streaming<WorkerResponse>,
    String,
) {
    let channel = Channel::from_shared(format!("http://{grpc_addr}"))
        .unwrap()
        .connect()
        .await
        .expect("Failed to connect gRPC channel to worker service");

    let mut client = worker_service_client::WorkerServiceClient::new(channel);

    let (tx, rx) = mpsc::channel::<WorkerRequest>(256);
    let outbound = ReceiverStream::new(rx);

    let response = client
        .session(outbound)
        .await
        .expect("Failed to start worker session");
    let inbound = response.into_inner();

    let worker_id = uuid::Uuid::now_v7().to_string();

    let hello = WorkerRequest {
        request: Some(worker_request::Request::Hello(WorkerHello {
            worker_id: worker_id.clone(),
            worker_name: "mock-test-worker".to_string(),
            queues: queues.iter().map(|s| s.to_string()).collect(),
            concurrency,
            metadata: String::new(),
        })),
    };
    tx.send(hello).await.expect("Failed to send WorkerHello");

    // Allow server to process the hello and start the match loop.
    tokio::time::sleep(Duration::from_millis(300)).await;

    (tx, inbound, worker_id)
}

/// Wait for a TaskAssignment on the worker response stream (with timeout).
async fn wait_for_task_assignment(
    inbound: &mut tonic::Streaming<WorkerResponse>,
    timeout_secs: u64,
) -> TaskAssignment {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match tokio::time::timeout_at(deadline, inbound.next()).await {
            Ok(Some(Ok(resp))) => {
                if let Some(worker_response::Response::TaskAssignment(assignment)) = resp.response {
                    return assignment;
                }
                // Skip other message types (heartbeat acks, etc.)
            }
            Ok(Some(Err(e))) => panic!("Stream error: {e}"),
            Ok(None) => panic!("Stream closed without receiving task assignment"),
            Err(_) => panic!("Timed out waiting for task assignment ({timeout_secs}s)"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Verify partition ownership is correctly split between two nodes.
#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_cluster_partition_ownership_split(pool: PgPool) {
    let num_partitions = 8;
    let queue = "ownership-queue";

    let node_a = TestNode::start(
        pool.clone(), "own-a", 18801, 19801, vec![18802], "test-own", num_partitions,
    )
    .await;
    let node_b = TestNode::start(
        pool, "own-b", 18802, 19802, vec![18801], "test-own", num_partitions,
    )
    .await;

    wait_for_members(&node_a.cluster, 2, 10).await;
    wait_for_members(&node_b.cluster, 2, 10).await;

    let a_owns = owned_partitions(&node_a.cluster, queue, num_partitions).await;
    let b_owns = owned_partitions(&node_b.cluster, queue, num_partitions).await;

    assert!(
        !a_owns.is_empty(),
        "Node A should own at least 1 partition, got 0"
    );
    assert!(
        !b_owns.is_empty(),
        "Node B should own at least 1 partition, got 0"
    );
    assert_eq!(
        a_owns.len() + b_owns.len(),
        num_partitions as usize,
        "All partitions must be accounted for (a={}, b={})",
        a_owns.len(),
        b_owns.len()
    );

    // No overlap
    for pid in &a_owns {
        assert!(!b_owns.contains(pid), "Partition {pid} owned by both nodes");
    }

    // Forwarding addresses are correct for non-owned partitions
    for pid in &b_owns {
        let addr = node_a.cluster.get_partition_owner_addr(queue, *pid).await;
        assert_eq!(
            addr.as_deref(),
            Some(node_b.grpc_addr.to_string().as_str()),
            "Node A should forward partition {pid} to Node B's gRPC address"
        );
    }
    for pid in &a_owns {
        let addr = node_b.cluster.get_partition_owner_addr(queue, *pid).await;
        assert_eq!(
            addr.as_deref(),
            Some(node_a.grpc_addr.to_string().as_str()),
            "Node B should forward partition {pid} to Node A's gRPC address"
        );
    }

    node_a.shutdown().await;
    node_b.shutdown().await;
}

/// Forwarding a task to the owner node when no worker is connected returns accepted=false.
#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_forward_task_no_worker_returns_not_accepted(pool: PgPool) {
    let num_partitions = 8;
    let queue = "noworker-queue";

    let node_a = TestNode::start(
        pool.clone(), "nw-a", 18811, 19811, vec![18812], "test-nw", num_partitions,
    )
    .await;
    let node_b = TestNode::start(
        pool.clone(), "nw-b", 18812, 19812, vec![18811], "test-nw", num_partitions,
    )
    .await;

    wait_for_members(&node_a.cluster, 2, 10).await;
    wait_for_members(&node_b.cluster, 2, 10).await;

    // Find a partition owned by Node B
    let b_owns = owned_partitions(&node_b.cluster, queue, num_partitions).await;
    assert!(!b_owns.is_empty(), "Node B should own at least 1 partition");

    let (task_id, partition_id) = find_task_for_partition(queue, &b_owns, num_partitions);
    insert_task(&pool, &task_id, queue, partition_id).await;

    // Forward from Node A to Node B — no worker listening, sync match should fail
    let result = node_a
        .forwarder
        .forward_task(&node_b.grpc_addr.to_string(), &task_id, queue, partition_id)
        .await;

    assert!(result.is_ok(), "forward_task should succeed, got: {result:?}");
    assert_eq!(
        result.unwrap(),
        false,
        "accepted should be false when no worker is waiting"
    );

    // Task should still be PENDING in PG
    let task = valka_db::queries::tasks::get_task(&pool, &task_id)
        .await
        .unwrap()
        .expect("Task should exist");
    assert_eq!(task.status, "PENDING");

    node_a.shutdown().await;
    node_b.shutdown().await;
}

/// Forward a task to a node that has a worker waiting — sync match succeeds.
#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_forward_task_with_waiting_worker(pool: PgPool) {
    let num_partitions = 8;
    let queue = "worker-queue";

    let node_a = TestNode::start(
        pool.clone(), "fw-a", 18821, 19821, vec![18822], "test-fw", num_partitions,
    )
    .await;
    let node_b = TestNode::start(
        pool.clone(), "fw-b", 18822, 19822, vec![18821], "test-fw", num_partitions,
    )
    .await;

    wait_for_members(&node_a.cluster, 2, 10).await;
    wait_for_members(&node_b.cluster, 2, 10).await;

    // Connect a worker to Node B
    let (_worker_tx, mut worker_stream, _worker_id) =
        connect_mock_worker(&node_b.grpc_addr, &[queue], 1).await;

    // Find a partition owned by Node B
    let b_owns = owned_partitions(&node_b.cluster, queue, num_partitions).await;
    let (task_id, partition_id) = find_task_for_partition(queue, &b_owns, num_partitions);
    insert_task(&pool, &task_id, queue, partition_id).await;

    // Forward from Node A → Node B
    let result = node_a
        .forwarder
        .forward_task(&node_b.grpc_addr.to_string(), &task_id, queue, partition_id)
        .await;

    assert!(result.is_ok(), "forward_task failed: {result:?}");
    assert_eq!(
        result.unwrap(),
        true,
        "accepted should be true when a worker is waiting"
    );

    // Worker on Node B should receive the task assignment
    let assignment = wait_for_task_assignment(&mut worker_stream, 5).await;
    assert_eq!(assignment.task_id, task_id);
    assert_eq!(assignment.queue_name, queue);
    assert_eq!(assignment.task_name, "cluster-test-task");

    node_a.shutdown().await;
    node_b.shutdown().await;
}

/// Full lifecycle: forward task → worker processes → sends result → task COMPLETED.
#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_cross_node_task_completion(pool: PgPool) {
    let num_partitions = 8;
    let queue = "complete-queue";

    let node_a = TestNode::start(
        pool.clone(), "cmp-a", 18831, 19831, vec![18832], "test-cmp", num_partitions,
    )
    .await;
    let node_b = TestNode::start(
        pool.clone(), "cmp-b", 18832, 19832, vec![18831], "test-cmp", num_partitions,
    )
    .await;

    wait_for_members(&node_a.cluster, 2, 10).await;
    wait_for_members(&node_b.cluster, 2, 10).await;

    // Connect worker to Node B
    let (worker_tx, mut worker_stream, _worker_id) =
        connect_mock_worker(&node_b.grpc_addr, &[queue], 1).await;

    // Create & forward task
    let b_owns = owned_partitions(&node_b.cluster, queue, num_partitions).await;
    let (task_id, partition_id) = find_task_for_partition(queue, &b_owns, num_partitions);
    insert_task(&pool, &task_id, queue, partition_id).await;

    let accepted = node_a
        .forwarder
        .forward_task(&node_b.grpc_addr.to_string(), &task_id, queue, partition_id)
        .await
        .expect("forward_task failed");
    assert!(accepted, "Task should be accepted by sync match");

    // Worker receives assignment
    let assignment = wait_for_task_assignment(&mut worker_stream, 5).await;
    assert_eq!(assignment.task_id, task_id);

    // Worker sends success result
    let result_msg = WorkerRequest {
        request: Some(worker_request::Request::TaskResult(TaskResult {
            task_id: assignment.task_id.clone(),
            task_run_id: assignment.task_run_id.clone(),
            success: true,
            retryable: false,
            output: r#"{"result":"done"}"#.to_string(),
            error_message: String::new(),
        })),
    };
    worker_tx
        .send(result_msg)
        .await
        .expect("Failed to send TaskResult");

    // Poll PG until COMPLETED
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let task = valka_db::queries::tasks::get_task(&pool, &task_id)
            .await
            .unwrap()
            .expect("Task should exist");
        if task.status == "COMPLETED" {
            assert_eq!(
                task.output,
                Some(serde_json::json!({"result": "done"})),
                "Task output should be preserved"
            );
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("Task did not reach COMPLETED status within 5s, status={}", task.status);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    node_a.shutdown().await;
    node_b.shutdown().await;
}

/// With 3 nodes, verify tasks get routed to the correct owner.
#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_three_node_task_routing(pool: PgPool) {
    let num_partitions = 12;
    let queue = "tri-queue";

    let node_a = TestNode::start(
        pool.clone(), "tri-a", 18841, 19841, vec![18842], "test-tri", num_partitions,
    )
    .await;
    let node_b = TestNode::start(
        pool.clone(), "tri-b", 18842, 19842, vec![18841], "test-tri", num_partitions,
    )
    .await;
    let node_c = TestNode::start(
        pool.clone(), "tri-c", 18843, 19843, vec![18841, 18842], "test-tri", num_partitions,
    )
    .await;

    wait_for_members(&node_a.cluster, 3, 10).await;
    wait_for_members(&node_b.cluster, 3, 10).await;
    wait_for_members(&node_c.cluster, 3, 10).await;

    let a_owns = owned_partitions(&node_a.cluster, queue, num_partitions).await;
    let b_owns = owned_partitions(&node_b.cluster, queue, num_partitions).await;
    let c_owns = owned_partitions(&node_c.cluster, queue, num_partitions).await;

    // All partitions covered, no overlap
    assert_eq!(
        a_owns.len() + b_owns.len() + c_owns.len(),
        num_partitions as usize,
        "All {num_partitions} partitions should be accounted for (a={}, b={}, c={})",
        a_owns.len(),
        b_owns.len(),
        c_owns.len()
    );

    // Connect a worker to each node
    let (_wa_tx, mut wa_stream, _) = connect_mock_worker(&node_a.grpc_addr, &[queue], 1).await;
    let (_wb_tx, mut wb_stream, _) = connect_mock_worker(&node_b.grpc_addr, &[queue], 1).await;
    let (_wc_tx, mut wc_stream, _) = connect_mock_worker(&node_c.grpc_addr, &[queue], 1).await;

    // Forward a task from Node A to Node B
    if !b_owns.is_empty() {
        let (task_id, pid) = find_task_for_partition(queue, &b_owns, num_partitions);
        insert_task(&pool, &task_id, queue, pid).await;

        let accepted = node_a
            .forwarder
            .forward_task(&node_b.grpc_addr.to_string(), &task_id, queue, pid)
            .await
            .expect("forward to B failed");
        assert!(accepted, "Node B should accept forwarded task");

        let assignment = wait_for_task_assignment(&mut wb_stream, 5).await;
        assert_eq!(assignment.task_id, task_id, "Worker on Node B should receive the task");
    }

    // Forward a task from Node A to Node C
    if !c_owns.is_empty() {
        let (task_id, pid) = find_task_for_partition(queue, &c_owns, num_partitions);
        insert_task(&pool, &task_id, queue, pid).await;

        let accepted = node_a
            .forwarder
            .forward_task(&node_c.grpc_addr.to_string(), &task_id, queue, pid)
            .await
            .expect("forward to C failed");
        assert!(accepted, "Node C should accept forwarded task");

        let assignment = wait_for_task_assignment(&mut wc_stream, 5).await;
        assert_eq!(assignment.task_id, task_id, "Worker on Node C should receive the task");
    }

    // Forward a task from Node B to Node A
    if !a_owns.is_empty() {
        let (task_id, pid) = find_task_for_partition(queue, &a_owns, num_partitions);
        insert_task(&pool, &task_id, queue, pid).await;

        let accepted = node_b
            .forwarder
            .forward_task(&node_a.grpc_addr.to_string(), &task_id, queue, pid)
            .await
            .expect("forward to A failed");
        assert!(accepted, "Node A should accept forwarded task");

        let assignment = wait_for_task_assignment(&mut wa_stream, 5).await;
        assert_eq!(assignment.task_id, task_id, "Worker on Node A should receive the task");
    }

    node_a.shutdown().await;
    node_b.shutdown().await;
    node_c.shutdown().await;
}

/// Test the full gRPC CreateTask API path — node auto-forwards if it doesn't own the partition.
#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_grpc_create_task_auto_forwards(pool: PgPool) {
    let num_partitions = 8;
    let queue = "auto-fwd-queue";

    let node_a = TestNode::start(
        pool.clone(), "af-a", 18851, 19851, vec![18852], "test-af", num_partitions,
    )
    .await;
    let node_b = TestNode::start(
        pool.clone(), "af-b", 18852, 19852, vec![18851], "test-af", num_partitions,
    )
    .await;

    wait_for_members(&node_a.cluster, 2, 10).await;
    wait_for_members(&node_b.cluster, 2, 10).await;

    // Connect a worker to Node B
    let (_worker_tx, mut worker_stream, _worker_id) =
        connect_mock_worker(&node_b.grpc_addr, &[queue], 4).await;

    // Use gRPC ApiService on Node A to create tasks.
    // Some tasks will land on partitions owned by Node A, others by Node B.
    let channel = Channel::from_shared(format!("http://{}", node_a.grpc_addr))
        .unwrap()
        .connect()
        .await
        .expect("Failed to connect gRPC channel to Node A");

    let mut api_client = api_service_client::ApiServiceClient::new(channel);

    // Determine which partitions Node B owns so we know what to look for.
    let b_owns = owned_partitions(&node_b.cluster, queue, num_partitions).await;
    assert!(!b_owns.is_empty(), "Node B should own at least 1 partition");

    // Create tasks until one is forwarded to Node B and received by its worker.
    // The gRPC CreateTask path on Node A will auto-detect non-owned partitions and forward.
    let mut forwarded_task_id = None;
    for i in 0..50 {
        let resp = api_client
            .create_task(CreateTaskRequest {
                queue_name: queue.to_string(),
                task_name: format!("auto-fwd-task-{i}"),
                input: r#"{"i":1}"#.to_string(),
                ..Default::default()
            })
            .await
            .expect("create_task failed");

        let task_meta = resp.into_inner().task.expect("task should be returned");
        let pid = partition_for_task(queue, &task_meta.id, num_partitions);

        if b_owns.contains(&pid.0) {
            forwarded_task_id = Some(task_meta.id);
            break;
        }
    }

    let forwarded_id = forwarded_task_id.expect(
        "After 50 tasks, at least one should have landed on a partition owned by Node B",
    );

    // Worker on Node B should receive the forwarded task
    let assignment = wait_for_task_assignment(&mut worker_stream, 5).await;
    assert_eq!(
        assignment.task_id, forwarded_id,
        "Worker on Node B should receive the auto-forwarded task"
    );

    node_a.shutdown().await;
    node_b.shutdown().await;
}
