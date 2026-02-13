use valka_cluster::forwarder::CircuitState;
use valka_cluster::ring::HashRing;
use valka_cluster::{ClusterManager, NodeForwarder};
use valka_core::NodeId;

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
        result.unwrap_err().to_string().contains("Circuit breaker open"),
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
