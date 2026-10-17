//! Tests against a real S3-compatible endpoint. Run with:
//!   VALKA_TEST_S3_ENDPOINT=http://localhost:9000 AWS_ACCESS_KEY_ID=minioadmin \
//!   AWS_SECRET_ACCESS_KEY=minioadmin cargo test -p valka-tests --features minio
//! The bucket `valka-test` must exist (docker compose creates it).

use std::time::Duration;

use valka_core::{StorageConfig, TaskStatus};
use valka_wal::Store;

use super::helpers::TestNode;

fn s3_store() -> Store {
    let endpoint = std::env::var("VALKA_TEST_S3_ENDPOINT").expect("VALKA_TEST_S3_ENDPOINT");
    let cfg = StorageConfig {
        backend: "s3".into(),
        bucket: std::env::var("VALKA_TEST_S3_BUCKET").unwrap_or_else(|_| "valka-test".into()),
        prefix: format!("test-{}", uuid::Uuid::now_v7()),
        endpoint: Some(endpoint),
        region: Some("us-east-1".into()),
        access_key_id: std::env::var("AWS_ACCESS_KEY_ID").ok(),
        secret_access_key: std::env::var("AWS_SECRET_ACCESS_KEY").ok(),
        allow_http: true,
        ..Default::default()
    };
    Store::from_config(&cfg).expect("s3 store")
}

#[tokio::test]
async fn minio_cas_and_conditional_get_are_real() {
    let s = s3_store();
    let etag = s
        .put_create("a", bytes::Bytes::from("1"))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        s.put_create("a", bytes::Bytes::from("x"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        s.put_if_match("a", &etag, bytes::Bytes::from("2"))
            .await
            .unwrap(),
        valka_wal::store::CasOutcome::Written(_)
    ));
    assert!(matches!(
        s.put_if_match("a", &etag, bytes::Bytes::from("3"))
            .await
            .unwrap(),
        valka_wal::store::CasOutcome::Conflict
    ));
}

#[tokio::test]
async fn minio_lifecycle_and_recovery() {
    let store = s3_store();
    let ids: Vec<String>;
    {
        let node = TestNode::on_store(store.clone(), "minio-node").await;
        let mut v = Vec::new();
        for _ in 0..5 {
            v.push(node.create("q", "t").await.id);
        }
        node.complete(&v[0]).await;
        node.engine.snapshot_now().await;
        v.push(node.create("q", "t").await.id);
        node.engine.sync().await.unwrap();
        ids = v;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let node = TestNode::on_store(store, "minio-node").await;
    assert_eq!(node.engine.list_tasks(None, None, 100, 0).len(), 6);
    assert_eq!(
        node.engine.get_task(&ids[0]).unwrap().status,
        TaskStatus::Completed
    );
    // Recovered PENDING tasks are runnable again: in the engine's index or already handed
    // to matching by the feeder.
    assert_eq!(
        node.engine.pending_count("q") + node.matching.buffered("q"),
        5
    );
}

#[tokio::test]
async fn minio_passes_the_storage_self_test() {
    valka_wal::meta::self_test(&s3_store(), "selftest-node")
        .await
        .expect("MinIO keeps the conditional-write and LIST promises");
}

/// Two engines on one real bucket: B tombstones and splits A after A dies, and every
/// acked task survives. Real time, short leases.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn minio_two_node_takeover() {
    use valka_engine::{CreateTask, Engine, EngineConfig, NoopSink, TokioClock};
    let store = s3_store();
    let cfg = |n: &str| {
        let mut c = EngineConfig::for_tests(n);
        c.cluster.num_shards = 16;
        c.cluster.lease_interval_ms = 300;
        c.cluster.lease_timeout_ms = 2_000;
        c.cluster.lease_safety_margin_ms = 700;
        c.cluster.membership_interval_ms = 200;
        c.cluster.placement_interval_ms = 200;
        c
    };
    let open = |n: &str| {
        Engine::open_with(
            store.clone(),
            cfg(n),
            TokioClock::new(),
            std::sync::Arc::new(NoopSink),
        )
    };
    let a = open("a").await.unwrap();
    let b = open("b").await.unwrap();
    let req = || CreateTask {
        queue_name: "q".into(),
        task_name: "t".into(),
        input: None,
        priority: 0,
        max_retries: 3,
        timeout_seconds: 60,
        idempotency_key: None,
        metadata: serde_json::json!({}),
        scheduled_at: None,
    };
    let mut ids = Vec::new();
    while ids.len() < 20 {
        for e in [&a, &b] {
            if let Ok(t) = e.create_task(req()).await {
                ids.push(t.id);
            }
        }
    }
    a.halt();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while b.owned_shards().len() < 16 {
        assert!(std::time::Instant::now() < deadline, "b never took over");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for id in &ids {
        assert!(b.get_task(id).is_some(), "acked task {id} lost");
    }
}
