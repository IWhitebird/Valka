use valka_cluster::ring::HashRing;

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
