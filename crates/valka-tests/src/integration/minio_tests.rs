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
    assert_eq!(node.engine.pending_count("q"), 5);
}
