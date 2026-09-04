//! `/api/v1/cluster/*` operator endpoints.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::helpers::*;

fn get_req(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn post_empty(uri: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn cluster_overview_reports_single_healthy_node() {
    let node = TestNode::new().await;
    let a = node.create("q", "t").await;
    node.create("other", "t").await;
    node.start(&a.id, "w");
    node.register_worker(&["q"], 2).await;
    node.engine.sync().await.unwrap();

    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["this_node"], "test-node");
    assert_eq!(body["num_shards"], 4096);
    assert_eq!(body["health"]["status"], "ok");
    assert_eq!(body["health"]["unowned_shards"], 0);
    let nodes = body["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    let n = &nodes[0];
    assert_eq!(n["node_id"], "test-node");
    assert_eq!(n["status"], "alive");
    assert_eq!(n["shards_owned"], 4096);
    assert_eq!(n["shards_with_tasks"].as_u64().unwrap() >= 1, true);
    assert_eq!(n["tasks"]["total"], 2);
    assert_eq!(n["tasks"]["running"], 1);
    assert_eq!(n["tasks"]["pending"], 1);
    assert_eq!(n["workers_connected"], 1);
    assert_eq!(n["queues"], serde_json::json!(["other", "q"]));
    assert_eq!(n["grpc_addr"], "127.0.0.1:50051");
    assert_eq!(n["storage_backend"], "memory");
    assert_eq!(n["wal"]["unflushed_records"], 0);
    assert!(n["wal"]["poisoned"].is_null());
    assert!(n["wal"]["durable_lsn"].as_str().unwrap().starts_with("1:"));
    assert!(n["started_at"].as_str().is_some());
    assert!(n["snapshots"]["dirty_shards"].as_u64().unwrap() >= 1);
}

#[tokio::test(start_paused = true)]
async fn shard_map_lists_all_shards_and_filters() {
    let node = TestNode::new().await;
    let a = node.create("q", "t").await;
    node.engine.sync().await.unwrap();
    let shard = valka_core::shard_of_task_id(&a.id).unwrap().0;

    let all = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster/shards"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(all.as_array().unwrap().len(), 4096);
    assert_eq!(all[0]["shard"], 0);
    assert_eq!(all[0]["owner"], "test-node");

    let dirty = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster/shards?dirty=true"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(dirty.as_array().unwrap().len(), 1);
    assert_eq!(dirty[0]["shard"], shard);
    assert_eq!(dirty[0]["tasks"], 1);
    assert_eq!(dirty[0]["pending"], 1);

    let none = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster/shards?node=someone-else"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(none, serde_json::json!([]));

    let with_tasks = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster/shards?min_tasks=1"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(with_tasks.as_array().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn shard_detail_and_out_of_range() {
    let node = TestNode::new().await;
    let a = node.create("q", "t").await;
    let shard = valka_core::shard_of_task_id(&a.id).unwrap().0;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req(&format!("/api/v1/cluster/shards/{shard}")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["shard"], shard);
    assert_eq!(body["queues"]["q"]["pending"], 1);
    assert_eq!(body["queues"]["q"]["total"], 1);
    let resp = node
        .router()
        .oneshot(get_req("/api/v1/cluster/shards/4096"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = node
        .router()
        .oneshot(get_req("/api/v1/cluster/shards/abc"))
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
}

#[tokio::test(start_paused = true)]
async fn storage_stats_and_snapshot_now() {
    let node = TestNode::new().await;
    for _ in 0..3 {
        node.create("q", "t").await;
    }
    node.engine.sync().await.unwrap();

    let s1 = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster/storage"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s1["backend"], "memory");
    assert!(s1["wal"]["segments"].as_u64().unwrap() >= 1);
    assert!(s1["wal"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(s1["snapshots"]["count"], 0);
    assert_eq!(s1["estimate"]["flush_interval_ms"], 5);
    assert!(s1["estimate"]["max_usd_per_day"].as_f64().unwrap() > 0.0);

    // Snapshot round: shards get snapshots, covered segments are truncated, cache is busted.
    let resp = node
        .router()
        .oneshot(post_empty("/api/v1/cluster/snapshot"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body["snapshots"]["dirty_shards"], 0);
    assert!(body["snapshots"]["last_round_at"].as_str().is_some());

    let s2 = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster/storage"))
            .await
            .unwrap(),
    )
    .await;
    assert!(s2["snapshots"]["count"].as_u64().unwrap() >= 1);
    assert!(s2["wal"]["segments"].as_u64().unwrap() <= 1);

    // Cached: same payload within the TTL.
    let s3 = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster/storage"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s2["computed_at"], s3["computed_at"]);
}

#[tokio::test(start_paused = true)]
async fn poisoned_writer_shows_critical_health() {
    let backing: std::sync::Arc<dyn object_store::ObjectStore> =
        std::sync::Arc::new(object_store::memory::InMemory::new());
    let (store, faults) = valka_wal::fault::faulty_over(backing, 1);
    let node = TestNode::on_store(store, "n").await;
    faults.set_puts_down(true);
    let _ = node.engine.create_task(task_req("q", "t")).await; // fails after retries
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["health"]["status"], "critical");
    assert_eq!(body["health"]["poisoned_nodes"], serde_json::json!(["n"]));
    assert_eq!(body["nodes"][0]["status"], "poisoned");
    assert!(body["nodes"][0]["wal"]["poisoned"].as_str().is_some());
    let resp = node.router().oneshot(get_req("/healthz")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
