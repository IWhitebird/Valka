//! Public API keys on the REST API, and the per-node limits (tasks in RAM, payload size).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use valka_engine::EngineConfig;
use valka_server::auth::ApiKeys;
use valka_wal::Store;

use super::helpers::*;

fn create_req(key: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri("/api/v1/tasks")
        .header("content-type", "application/json");
    if let Some(k) = key {
        b = b.header("authorization", format!("Bearer {k}"));
    }
    b.body(Body::from(r#"{"queue_name":"q","task_name":"t"}"#))
        .unwrap()
}

fn get_req(uri: &str, key: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(uri);
    if let Some(k) = key {
        b = b.header("authorization", format!("Bearer {k}"));
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test(start_paused = true)]
async fn without_keys_the_api_is_open() {
    let node = TestNode::new().await;
    let r = node.router().oneshot(create_req(None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
}

#[tokio::test(start_paused = true)]
async fn api_keys_guard_the_public_api() {
    let node = TestNode::new().await;
    let router = || node.router_with_keys(Some("admin-secret"), ApiKeys::parse(Some("k1,k2")));

    // No key, a wrong key: refused with a Bearer challenge.
    let r = router().oneshot(create_req(None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(r.headers()["www-authenticate"], "Bearer");
    let body = parse_response_json(r).await;
    assert_eq!(body["code"], "UNAUTHENTICATED");
    let r = router().oneshot(create_req(Some("nope"))).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // Either configured key works.
    let r = router().oneshot(create_req(Some("k1"))).await.unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let r = router().oneshot(create_req(Some("k2"))).await.unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let r = router()
        .oneshot(get_req("/api/v1/tasks", Some("k1")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = router()
        .oneshot(get_req("/api/v1/cluster", None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "cluster views too");

    // Health and metrics stay open for probes and scrapers.
    for open in ["/healthz", "/metrics"] {
        let r = router().oneshot(get_req(open, None)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{open}");
    }

    // The admin API keeps its own token, not an API key.
    let r = router()
        .oneshot(get_req("/api/v1/cluster/audit", Some("admin-secret")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = router()
        .oneshot(get_req("/api/v1/cluster/audit", Some("k1")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // The event stream also takes the key as ?access_token= (EventSource sends no headers);
    // other endpoints do not.
    let r = router()
        .oneshot(get_req("/api/v1/events?access_token=k2", None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = router()
        .oneshot(get_req("/api/v1/tasks?access_token=k2", None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

async fn limited_node(max_tasks: usize, max_payload: usize) -> TestNode {
    let mut cfg = EngineConfig::for_tests("limited");
    cfg.limits.max_tasks_in_ram = max_tasks;
    cfg.limits.max_payload_bytes = max_payload;
    TestNode::build(Store::memory(), cfg, None, None).await
}

#[tokio::test(start_paused = true)]
async fn creates_are_refused_at_the_task_limit() {
    let node = limited_node(3, 1024).await;
    for _ in 0..3 {
        let r = node.router().oneshot(create_req(None)).await.unwrap();
        assert_eq!(r.status(), StatusCode::CREATED);
    }
    let r = node.router().oneshot(create_req(None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = parse_response_json(r).await;
    assert_eq!(body["code"], "RESOURCE_EXHAUSTED");
    assert!(body["error"].as_str().unwrap().contains("max_tasks_in_ram"));

    // Finished tasks leave RAM after their retention (early, under pressure): once the
    // backlog is worked off and a minute has passed, creates are accepted again.
    for t in node.engine.list_tasks(None, None, 10, 0) {
        let run = node.start(&t.id, "w");
        node.engine.complete_run(&t.id, &run, None).await.unwrap();
    }
    tokio::time::advance(std::time::Duration::from_secs(65)).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let r = node.router().oneshot(create_req(None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
}

#[tokio::test(start_paused = true)]
async fn oversized_payloads_are_refused() {
    let node = limited_node(100, 64).await;
    let big = "x".repeat(100);
    let r = node
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tasks")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"queue_name": "q", "task_name": "t", "input": {"blob": big}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let body = parse_response_json(r).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("max_payload_bytes")
    );
    let small = node.router().oneshot(create_req(None)).await.unwrap();
    assert_eq!(small.status(), StatusCode::CREATED);
}
