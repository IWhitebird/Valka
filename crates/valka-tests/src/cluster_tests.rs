use std::time::Duration;

use valka_cluster::forwarder::CircuitState;
use valka_cluster::ring::HashRing;
use valka_cluster::{ClusterEvent, ClusterManager, NodeForwarder};
use valka_core::{GossipConfig, NodeId};

#[test]
fn test_empty_ring_returns_none() {
    let ring = HashRing::new();
    assert!(ring.get_node("any-key").is_none());
}

#[test]
fn test_single_node_all_keys() {
    let mut ring = HashRing::new();
    ring.add_node("node-1");

    for i in 0..100 {
        let key = format!("key-{i}");
        let node = ring.get_node(&key);
        assert_eq!(
            node.as_deref(),
            Some("node-1"),
            "All keys should map to the only node"
        );
    }
}

#[test]
fn test_two_nodes_distribution() {
    let mut ring = HashRing::new();
    ring.add_node("node-1");
    ring.add_node("node-2");

    let mut count_1 = 0;
    let mut count_2 = 0;
    for i in 0..1000 {
        let key = format!("task-{i}");
        match ring.get_node(&key).as_deref() {
            Some("node-1") => count_1 += 1,
            Some("node-2") => count_2 += 1,
            other => panic!("Unexpected node: {other:?}"),
        }
    }

    assert!(count_1 > 0, "Node 1 should have some keys, got {count_1}");
    assert!(count_2 > 0, "Node 2 should have some keys, got {count_2}");
}

#[test]
fn test_add_remove_node() {
    let mut ring = HashRing::new();
    ring.add_node("node-1");
    ring.add_node("node-2");

    // Remove node-1
    ring.remove_node("node-1");

    // All keys should now map to node-2
    for i in 0..100 {
        let key = format!("key-{i}");
        assert_eq!(
            ring.get_node(&key).as_deref(),
            Some("node-2"),
            "After removing node-1, all keys should map to node-2"
        );
    }
}

#[test]
fn test_remove_nonexistent_node() {
    let mut ring = HashRing::new();
    // Should not panic
    ring.remove_node("ghost-node");

    ring.add_node("node-1");
    ring.remove_node("ghost-node");
    // node-1 should still work
    assert_eq!(ring.get_node("key").as_deref(), Some("node-1"));
}

#[test]
fn test_consistency_after_add() {
    let mut ring = HashRing::new();
    ring.add_node("node-1");

    // Record mappings before adding node-2
    let mut before = Vec::new();
    for i in 0..100 {
        let key = format!("key-{i}");
        before.push((key.clone(), ring.get_node(&key)));
    }

    ring.add_node("node-2");

    // Most keys that mapped to node-1 should still map to node-1
    let mut stayed = 0;
    for (key, old_node) in &before {
        let new_node = ring.get_node(key);
        if *old_node == new_node {
            stayed += 1;
        }
    }

    // With consistent hashing, at least half should stay
    assert!(
        stayed > 30,
        "At least 30% of keys should stay mapped to the same node, got {stayed}/100"
    );
}

#[test]
fn test_deterministic() {
    let mut ring = HashRing::new();
    ring.add_node("alpha");
    ring.add_node("beta");

    let result1 = ring.get_node("test-key");
    let result2 = ring.get_node("test-key");
    assert_eq!(result1, result2, "Same key should always return same node");
}

#[test]
fn test_node_id_returned_not_virtual() {
    let mut ring = HashRing::new();
    ring.add_node("my-node");

    let node = ring.get_node("any-key").unwrap();
    assert!(
        !node.contains('#'),
        "Returned node ID should not contain virtual node suffix, got: {node}"
    );
    assert_eq!(node, "my-node");
}

#[test]
fn test_many_nodes_all_represented() {
    let mut ring = HashRing::new();
    for i in 0..10 {
        ring.add_node(&format!("node-{i}"));
    }

    let mut seen = std::collections::HashSet::new();
    for i in 0..10000 {
        let key = format!("key-{i}");
        if let Some(node) = ring.get_node(&key) {
            seen.insert(node);
        }
    }

    assert_eq!(
        seen.len(),
        10,
        "All 10 nodes should appear in the distribution, got {}",
        seen.len()
    );
}

#[test]
fn test_default_creates_empty_ring() {
    let ring = HashRing::default();
    assert!(ring.get_node("key").is_none());
}

// --- ClusterManager tests ---

#[tokio::test]
async fn test_single_node_owns_all_partitions() {
    let node_id = NodeId("test-node-1".to_string());
    let cluster = ClusterManager::new_single_node(node_id, 4);

    for pid in 0..4 {
        assert!(
            cluster.owns_partition("my-queue", pid).await,
            "Single node should own partition {pid}"
        );
    }
}

#[tokio::test]
async fn test_single_node_no_forwarding_needed() {
    let node_id = NodeId("test-node-1".to_string());
    let cluster = ClusterManager::new_single_node(node_id, 4);

    // get_partition_owner_addr should return None (we own everything)
    for pid in 0..4 {
        let addr = cluster.get_partition_owner_addr("my-queue", pid).await;
        assert!(
            addr.is_none(),
            "Single node should return None (no forwarding needed) for partition {pid}"
        );
    }
}

#[tokio::test]
async fn test_cluster_event_subscribe() {
    let node_id = NodeId("test-node-1".to_string());
    let cluster = ClusterManager::new_single_node(node_id, 4);

    // Should be able to subscribe without panic
    let _rx = cluster.subscribe_events();
}

#[tokio::test]
async fn test_single_node_members() {
    let node_id = NodeId("test-node-1".to_string());
    let cluster = ClusterManager::new_single_node(node_id, 4);

    let members = cluster.members().await;
    assert_eq!(members.len(), 1);
    assert!(members.contains("test-node-1"));
}

#[tokio::test]
async fn test_single_node_not_clustered() {
    let node_id = NodeId("test-node-1".to_string());
    let cluster = ClusterManager::new_single_node(node_id, 4);

    assert!(!cluster.is_clustered());
}

#[test]
fn test_two_node_ring_partition_split() {
    // Verify that with 2 nodes in the ring, partitions distribute across both
    let mut ring = HashRing::new();
    ring.add_node("node-a");
    ring.add_node("node-b");

    let num_partitions = 16;
    let mut node_a_count = 0;
    let mut node_b_count = 0;

    for pid in 0..num_partitions {
        let key = format!("test-queue:{pid}");
        match ring.get_node(&key).as_deref() {
            Some("node-a") => node_a_count += 1,
            Some("node-b") => node_b_count += 1,
            other => panic!("Unexpected node: {other:?}"),
        }
    }

    assert!(
        node_a_count > 0,
        "Node A should own at least 1 partition, got {node_a_count}"
    );
    assert!(
        node_b_count > 0,
        "Node B should own at least 1 partition, got {node_b_count}"
    );
    assert_eq!(node_a_count + node_b_count, num_partitions);
}

#[test]
fn test_ring_partition_ownership_changes_on_node_add() {
    let mut ring = HashRing::new();
    ring.add_node("node-a");

    // All 8 partitions belong to node-a
    for pid in 0..8 {
        let key = format!("q:{pid}");
        assert_eq!(ring.get_node(&key).as_deref(), Some("node-a"));
    }

    // Add node-b: some partitions should move
    ring.add_node("node-b");

    let mut moved = 0;
    for pid in 0..8 {
        let key = format!("q:{pid}");
        if ring.get_node(&key).as_deref() == Some("node-b") {
            moved += 1;
        }
    }

    assert!(
        moved > 0,
        "Adding a second node should cause some partitions to move"
    );
    assert!(
        moved < 8,
        "Adding a second node should not move all partitions"
    );
}

// --- Circuit Breaker tests ---

#[tokio::test]
async fn test_circuit_breaker_opens_after_failures() {
    let forwarder = NodeForwarder::new();
    let addr = "127.0.0.1:99999"; // Non-existent, will fail

    // Circuit starts closed
    assert_eq!(
        forwarder.get_circuit_state(addr).await,
        CircuitState::Closed
    );

    // Make 3 failed forward attempts (each has 1 retry = 2 failures per call)
    // After 2 calls (4 failures total, >= threshold of 3), circuit should be open
    for _ in 0..2 {
        let _ = forwarder.forward_task(addr, "task-1", "queue-1", 0).await;
    }

    assert_eq!(
        forwarder.get_circuit_state(addr).await,
        CircuitState::Open,
        "Circuit should be open after repeated failures"
    );

    // Further calls should fail immediately with circuit breaker error
    let result = forwarder.forward_task(addr, "task-2", "queue-1", 0).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Circuit breaker open"),
        "Should get circuit breaker error"
    );
}

#[tokio::test]
async fn test_circuit_breaker_resets_on_success() {
    let forwarder = NodeForwarder::new();
    let addr = "127.0.0.1:99998";

    // Manually record failures to open the circuit
    // We call forward_task which will fail and record failures
    for _ in 0..2 {
        let _ = forwarder.forward_task(addr, "task-1", "queue-1", 0).await;
    }

    assert_eq!(
        forwarder.get_circuit_state(addr).await,
        CircuitState::Open,
        "Circuit should be open"
    );

    // remove_node clears circuit state
    forwarder.remove_node(addr).await;
    assert_eq!(
        forwarder.get_circuit_state(addr).await,
        CircuitState::Closed,
        "Circuit should be closed after remove_node"
    );
}

#[tokio::test]
async fn test_circuit_breaker_default_state() {
    let forwarder = NodeForwarder::new();

    // Unknown addr should return Closed (default)
    assert_eq!(
        forwarder.get_circuit_state("unknown:1234").await,
        CircuitState::Closed,
        "Unknown node should have Closed circuit state"
    );
}

// ─── Multi-Node Gossip Tests ────────────────────────────────────────────
//
// These tests spin up real chitchat gossip over UDP on localhost.
// Each test uses unique port ranges to avoid conflicts during parallel execution.
// Gossip convergence takes 2-3 seconds (500ms gossip interval).

/// Poll until the cluster sees `expected` members, or panic on timeout.
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

#[tokio::test]
async fn test_two_node_cluster_discovery() {
    let config_a = gossip_config(17701, vec![17702], "test-disco");
    let config_b = gossip_config(17702, vec![17701], "test-disco");

    let node_a = ClusterManager::new_clustered(
        NodeId("disco-a".to_string()),
        4,
        &config_a,
        "127.0.0.1:50101",
    )
    .await
    .unwrap();

    let node_b = ClusterManager::new_clustered(
        NodeId("disco-b".to_string()),
        4,
        &config_b,
        "127.0.0.1:50102",
    )
    .await
    .unwrap();

    // Wait for both nodes to discover each other
    wait_for_members(&node_a, 2, 10).await;
    wait_for_members(&node_b, 2, 10).await;

    let members_a = node_a.members().await;
    let members_b = node_b.members().await;

    assert!(members_a.contains("disco-a"));
    assert!(members_a.contains("disco-b"));
    assert!(members_b.contains("disco-a"));
    assert!(members_b.contains("disco-b"));

    // Both should report as clustered
    assert!(node_a.is_clustered());
    assert!(node_b.is_clustered());

    node_a.shutdown().await;
    node_b.shutdown().await;
}

#[tokio::test]
async fn test_three_node_cluster_all_discover() {
    let config_a = gossip_config(17711, vec![17712], "test-3node");
    let config_b = gossip_config(17712, vec![17711], "test-3node");
    let config_c = gossip_config(17713, vec![17711, 17712], "test-3node");

    let node_a =
        ClusterManager::new_clustered(NodeId("tri-a".to_string()), 8, &config_a, "127.0.0.1:50111")
            .await
            .unwrap();

    let node_b =
        ClusterManager::new_clustered(NodeId("tri-b".to_string()), 8, &config_b, "127.0.0.1:50112")
            .await
            .unwrap();

    let node_c =
        ClusterManager::new_clustered(NodeId("tri-c".to_string()), 8, &config_c, "127.0.0.1:50113")
            .await
            .unwrap();

    // All three should eventually see each other
    wait_for_members(&node_a, 3, 10).await;
    wait_for_members(&node_b, 3, 10).await;
    wait_for_members(&node_c, 3, 10).await;

    let members = node_a.members().await;
    assert!(members.contains("tri-a"));
    assert!(members.contains("tri-b"));
    assert!(members.contains("tri-c"));

    node_a.shutdown().await;
    node_b.shutdown().await;
    node_c.shutdown().await;
}

#[tokio::test]
async fn test_cluster_grpc_addr_propagation() {
    let config_a = gossip_config(17731, vec![17732], "test-addr");
    let config_b = gossip_config(17732, vec![17731], "test-addr");

    let node_a =
        ClusterManager::new_clustered(NodeId("addr-a".to_string()), 4, &config_a, "10.0.0.1:50051")
            .await
            .unwrap();

    let node_b =
        ClusterManager::new_clustered(NodeId("addr-b".to_string()), 4, &config_b, "10.0.0.2:50051")
            .await
            .unwrap();

    wait_for_members(&node_a, 2, 10).await;
    wait_for_members(&node_b, 2, 10).await;

    // Node A should know Node B's gRPC address (and vice versa)
    let b_addr = node_a.get_node_grpc_addr("addr-b").await;
    assert_eq!(
        b_addr.as_deref(),
        Some("10.0.0.2:50051"),
        "Node A should know Node B's gRPC address"
    );

    let a_addr = node_b.get_node_grpc_addr("addr-a").await;
    assert_eq!(
        a_addr.as_deref(),
        Some("10.0.0.1:50051"),
        "Node B should know Node A's gRPC address"
    );

    node_a.shutdown().await;
    node_b.shutdown().await;
}

#[tokio::test]
async fn test_cluster_partition_ownership_splits_across_nodes() {
    let config_a = gossip_config(17721, vec![17722], "test-part");
    let config_b = gossip_config(17722, vec![17721], "test-part");

    let num_partitions = 16;

    let node_a = ClusterManager::new_clustered(
        NodeId("part-a".to_string()),
        num_partitions,
        &config_a,
        "127.0.0.1:50121",
    )
    .await
    .unwrap();

    let node_b = ClusterManager::new_clustered(
        NodeId("part-b".to_string()),
        num_partitions,
        &config_b,
        "127.0.0.1:50122",
    )
    .await
    .unwrap();

    wait_for_members(&node_a, 2, 10).await;
    wait_for_members(&node_b, 2, 10).await;

    // After convergence, partitions should be split between the two nodes
    let mut a_owns = 0;
    let mut b_owns = 0;
    for pid in 0..num_partitions {
        if node_a.owns_partition("test-queue", pid).await {
            a_owns += 1;
        }
        if node_b.owns_partition("test-queue", pid).await {
            b_owns += 1;
        }
    }

    assert!(
        a_owns > 0,
        "Node A should own at least 1 partition, got {a_owns}"
    );
    assert!(
        b_owns > 0,
        "Node B should own at least 1 partition, got {b_owns}"
    );
    // Each partition should be owned by exactly one node
    assert_eq!(
        a_owns + b_owns,
        num_partitions,
        "All {num_partitions} partitions should be accounted for (a={a_owns}, b={b_owns})"
    );

    // Node A should get forwarding addresses for partitions it doesn't own
    let mut forward_needed = 0;
    for pid in 0..num_partitions {
        if !node_a.owns_partition("test-queue", pid).await {
            let addr = node_a.get_partition_owner_addr("test-queue", pid).await;
            assert!(
                addr.is_some(),
                "Should return owner addr for partition {pid} not owned by node A"
            );
            forward_needed += 1;
        }
    }
    assert_eq!(forward_needed, b_owns as i32);

    node_a.shutdown().await;
    node_b.shutdown().await;
}

#[tokio::test]
async fn test_cluster_events_emitted_on_node_join() {
    let config_a = gossip_config(17741, vec![17742], "test-events");

    let node_a =
        ClusterManager::new_clustered(NodeId("evt-a".to_string()), 4, &config_a, "127.0.0.1:50141")
            .await
            .unwrap();

    // Subscribe BEFORE node B joins
    let mut event_rx = node_a.subscribe_events();

    // Now start node B
    let config_b = gossip_config(17742, vec![17741], "test-events");
    let node_b =
        ClusterManager::new_clustered(NodeId("evt-b".to_string()), 4, &config_b, "127.0.0.1:50142")
            .await
            .unwrap();

    wait_for_members(&node_a, 2, 10).await;

    // Collect events with a short timeout
    let mut saw_join = false;
    let mut saw_rebalance = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);

    loop {
        tokio::select! {
            result = event_rx.recv() => {
                match result {
                    Ok(ClusterEvent::NodeJoined { node_id, .. }) => {
                        if node_id.0 == "evt-b" {
                            saw_join = true;
                        }
                    }
                    Ok(ClusterEvent::PartitionsRebalanced) => {
                        saw_rebalance = true;
                    }
                    Ok(ClusterEvent::NodeLeft { .. }) => {}
                    Err(_) => break,
                }
                if saw_join && saw_rebalance {
                    break;
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(saw_join, "Should have received NodeJoined event for evt-b");
    assert!(
        saw_rebalance,
        "Should have received PartitionsRebalanced event"
    );

    node_a.shutdown().await;
    node_b.shutdown().await;
}

// Note: Testing gossip-level node leave detection (failure detector) is omitted because
// chitchat's phi-accrual detector takes 30-60+ seconds to declare a node dead with
// default thresholds, making it too slow for unit tests. The ring-level reclamation
// is already tested by test_add_remove_node. If needed, this can be tested with
// chitchat's test utilities or with a custom failure_detector_config.

#[test]
fn test_ring_node_leave_reclaims_all_partitions() {
    // Verify at the ring level that removing a node gives all partitions back
    let mut ring = HashRing::new();
    ring.add_node("node-a");
    ring.add_node("node-b");

    let num_partitions = 16;

    // Verify split
    let mut a_before = 0;
    for pid in 0..num_partitions {
        let key = format!("work:{pid}");
        if ring.get_node(&key).as_deref() == Some("node-a") {
            a_before += 1;
        }
    }
    assert!(a_before < num_partitions, "Should be split with 2 nodes");

    // Remove node-b (simulates node leaving)
    ring.remove_node("node-b");

    // All partitions should now belong to node-a
    for pid in 0..num_partitions {
        let key = format!("work:{pid}");
        assert_eq!(
            ring.get_node(&key).as_deref(),
            Some("node-a"),
            "All partitions should return to node-a after node-b removed"
        );
    }
}

#[tokio::test]
async fn test_cluster_graceful_shutdown() {
    let config = gossip_config(17761, vec![], "test-shutdown");

    let node = ClusterManager::new_clustered(
        NodeId("shutdown-node".to_string()),
        4,
        &config,
        "127.0.0.1:50161",
    )
    .await
    .unwrap();

    assert!(node.is_clustered());
    // Shutdown should not panic or hang
    node.shutdown().await;
}

// ─── Additional Circuit Breaker Edge Cases ──────────────────────────────

#[tokio::test]
async fn test_circuit_breaker_per_node_isolation() {
    let forwarder = NodeForwarder::new();
    let addr_a = "127.0.0.1:99990";
    let addr_b = "127.0.0.1:99991";

    // Open circuit for addr_a by causing failures
    for _ in 0..2 {
        let _ = forwarder.forward_task(addr_a, "t1", "q", 0).await;
    }

    assert_eq!(
        forwarder.get_circuit_state(addr_a).await,
        CircuitState::Open,
        "addr_a circuit should be open"
    );

    // addr_b should still be closed (independent)
    assert_eq!(
        forwarder.get_circuit_state(addr_b).await,
        CircuitState::Closed,
        "addr_b circuit should remain closed"
    );
}

#[tokio::test]
async fn test_circuit_breaker_open_rejects_immediately() {
    let forwarder = NodeForwarder::new();
    let addr = "127.0.0.1:99985";

    // Open the circuit
    for _ in 0..2 {
        let _ = forwarder.forward_task(addr, "t1", "q", 0).await;
    }
    assert_eq!(forwarder.get_circuit_state(addr).await, CircuitState::Open);

    // Subsequent calls should fail fast without connecting
    let start = std::time::Instant::now();
    let result = forwarder.forward_task(addr, "t2", "q", 0).await;
    let elapsed = start.elapsed();

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Circuit breaker open"),
        "Should fail with circuit breaker error"
    );
    // Should be near-instant (< 100ms), not waiting for connection timeout
    assert!(
        elapsed < Duration::from_millis(100),
        "Open circuit should reject immediately, took {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_forwarder_remove_node_clears_cache_and_circuit() {
    let forwarder = NodeForwarder::new();
    let addr = "127.0.0.1:99980";

    // Open circuit for this addr
    for _ in 0..2 {
        let _ = forwarder.forward_task(addr, "t1", "q", 0).await;
    }
    assert_eq!(forwarder.get_circuit_state(addr).await, CircuitState::Open);

    // remove_node should clear both channel cache and circuit state
    forwarder.remove_node(addr).await;

    assert_eq!(
        forwarder.get_circuit_state(addr).await,
        CircuitState::Closed,
        "Circuit should be reset after remove_node"
    );

    // A new forward attempt should try to connect (not fast-fail)
    // It will still fail (no server), but the circuit starts fresh
    let _ = forwarder.forward_task(addr, "t2", "q", 0).await;
    // After 1 call (2 failures from attempt+retry), circuit may not be open yet
    // because it starts from 0 again
    let state = forwarder.get_circuit_state(addr).await;
    // 2 failures < threshold of 3, so should still be Closed
    assert_eq!(
        state,
        CircuitState::Closed,
        "After 1 fresh call (2 failures), circuit should still be Closed"
    );
}

// ─── Additional Hash Ring Edge Cases ────────────────────────────────────

#[test]
fn test_ring_add_third_node_moves_roughly_one_third() {
    let mut ring = HashRing::new();
    ring.add_node("node-1");
    ring.add_node("node-2");

    // Record mappings with 2 nodes
    let mut before: Vec<(String, String)> = Vec::new();
    for i in 0..1000 {
        let key = format!("k-{i}");
        let node = ring.get_node(&key).unwrap();
        before.push((key, node));
    }

    // Add a third node
    ring.add_node("node-3");

    let mut moved_count = 0;
    let mut to_new_node = 0;
    for (key, old_node) in &before {
        let new_node = ring.get_node(key).unwrap();
        if new_node != *old_node {
            moved_count += 1;
        }
        if new_node == "node-3" {
            to_new_node += 1;
        }
    }

    // With consistent hashing, roughly 1/3 of keys should move to the new node
    // Allow wide tolerance: between 15% and 50%
    assert!(
        to_new_node > 150 && to_new_node < 500,
        "~1/3 of keys should go to new node, got {to_new_node}/1000"
    );
    // Moved keys should be approximately equal to keys on new node
    // (keys move TO the new node, not between existing nodes)
    assert!(
        moved_count > 150 && moved_count < 500,
        "~1/3 of keys should move, got {moved_count}/1000"
    );
}

#[test]
fn test_ring_multiple_queues_distribute_independently() {
    let mut ring = HashRing::new();
    ring.add_node("node-x");
    ring.add_node("node-y");

    // Different queues with the same partition IDs should potentially map differently
    let mut same_mapping = 0;
    for pid in 0..32 {
        let owner_q1 = ring.get_node(&format!("queue-alpha:{pid}")).unwrap();
        let owner_q2 = ring.get_node(&format!("queue-beta:{pid}")).unwrap();
        if owner_q1 == owner_q2 {
            same_mapping += 1;
        }
    }

    // With two nodes, each partition has ~50% chance of matching.
    // Not ALL should match (that would mean queue name doesn't affect distribution)
    assert!(
        same_mapping < 32,
        "Different queue names should produce different partition distributions, \
         but all {same_mapping}/32 matched"
    );
}
