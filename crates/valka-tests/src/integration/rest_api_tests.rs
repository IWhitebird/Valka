use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Duration;
use tower::ServiceExt;
use valka_core::TaskStatus;

use super::helpers::*;

fn delete_req(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(json_body(body)))
        .unwrap()
}

fn post_empty(uri: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn get_req(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

// ─── POST /api/v1/tasks ─────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn test_rest_create_task() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "demo", "task_name": "email.send", "input": {"to": "user@example.com"}}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = parse_response_json(resp).await;
    assert_eq!(body["queue_name"], "demo");
    assert_eq!(body["task_name"], "email.send");
    assert_eq!(body["status"], "PENDING");
    assert_eq!(body["input"]["to"], "user@example.com");
    assert!(!body["id"].as_str().unwrap().is_empty());
    // Durable in the bucket.
    node.engine.sync().await.unwrap();
    assert_eq!(
        valka_wal::reader::read_all(&node.store, "test-node", None)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_minimal() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "q", "task_name": "t"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = parse_response_json(resp).await;
    assert_eq!(body["status"], "PENDING");
    assert!(body["input"].is_null());
    assert_eq!(body["metadata"], serde_json::json!({}));
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_all_fields() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({
                "queue_name": "billing", "task_name": "charge", "input": {"amount": 100},
                "priority": 10, "max_retries": 5, "timeout_seconds": 600,
                "idempotency_key": "idem-001", "metadata": {"source": "api"}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = parse_response_json(resp).await;
    assert_eq!(body["priority"], 10);
    assert_eq!(body["max_retries"], 5);
    assert_eq!(body["timeout_seconds"], 600);
    assert_eq!(body["idempotency_key"], "idem-001");
    assert_eq!(body["metadata"]["source"], "api");
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_with_scheduled_at() {
    let node = TestNode::new().await;
    let future = (node.engine.clock().now() + Duration::hours(1)).to_rfc3339();
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "q", "task_name": "t", "scheduled_at": future}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = parse_response_json(resp).await;
    assert_eq!(body["status"], "PENDING");
    assert!(body["scheduled_at"].as_str().is_some());
    // Not runnable yet.
    assert_eq!(node.engine.pending_count("q"), 0);
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_defaults() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "q", "task_name": "t"}),
        ))
        .await
        .unwrap();
    let body = parse_response_json(resp).await;
    assert_eq!(body["priority"], 0);
    assert_eq!(body["max_retries"], 3);
    assert_eq!(body["timeout_seconds"], 300);
    assert_eq!(body["attempt_count"], 0);
    assert!(body["idempotency_key"].is_null());
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_missing_queue_name() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({"task_name": "t"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_missing_task_name() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "q"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_empty_names_rejected() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "", "task_name": ""}),
        ))
        .await
        .unwrap();
    assert_error_response(resp, StatusCode::BAD_REQUEST, "BAD_REQUEST", "required").await;
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_empty_body() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_empty("/api/v1/tasks"))
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_idempotency_conflict() {
    let node = TestNode::new().await;
    let body = serde_json::json!({"queue_name": "q", "task_name": "t", "idempotency_key": "same"});
    let r1 = node
        .router()
        .oneshot(post_json("/api/v1/tasks", body.clone()))
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::CREATED);
    let r2 = node
        .router()
        .oneshot(post_json("/api/v1/tasks", body))
        .await
        .unwrap();
    assert_error_response(r2, StatusCode::CONFLICT, "CONFLICT", "same").await;
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_negative_priority() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "q", "task_name": "t", "priority": -5}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(parse_response_json(resp).await["priority"], -5);
}

#[tokio::test(start_paused = true)]
async fn test_rest_create_task_invalid_scheduled_at() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "q", "task_name": "t", "scheduled_at": "not-a-date"}),
        ))
        .await
        .unwrap();
    assert_error_response(resp, StatusCode::BAD_REQUEST, "BAD_REQUEST", "scheduled_at").await;
}

// ─── GET /api/v1/tasks/:id ──────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn test_rest_get_task() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let resp = node
        .router()
        .oneshot(get_req(&format!("/api/v1/tasks/{}", t.id)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body["id"], t.id);
    assert_eq!(body["status"], "PENDING");
    assert_eq!(body["input"]["key"], "value");
}

#[tokio::test(start_paused = true)]
async fn test_rest_get_task_not_found() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(get_req("/api/v1/tasks/does-not-exist"))
        .await
        .unwrap();
    assert_error_response(resp, StatusCode::NOT_FOUND, "NOT_FOUND", "not found").await;
    let resp = node
        .router()
        .oneshot(get_req(
            "/api/v1/tasks/00000000-0000-7000-8000-000000000001",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ─── GET /api/v1/tasks ──────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn test_rest_list_tasks() {
    let node = TestNode::new().await;
    for i in 0..3 {
        node.create("q", &format!("t{i}")).await;
    }
    let resp = node
        .router()
        .oneshot(get_req("/api/v1/tasks"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn test_rest_list_tasks_filter_queue() {
    let node = TestNode::new().await;
    node.create("a", "t").await;
    node.create("a", "t").await;
    node.create("b", "t").await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks?queue_name=a"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 2);
    assert!(
        body.as_array()
            .unwrap()
            .iter()
            .all(|t| t["queue_name"] == "a")
    );
}

#[tokio::test(start_paused = true)]
async fn test_rest_list_tasks_filter_status() {
    let node = TestNode::new().await;
    let a = node.create("q", "t").await;
    node.create("q", "t").await;
    node.complete(&a.id).await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks?status=COMPLETED"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], a.id);
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks?status=PENDING"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    let resp = node
        .router()
        .oneshot(get_req("/api/v1/tasks?status=BOGUS"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(start_paused = true)]
async fn test_rest_list_tasks_pagination() {
    let node = TestNode::new().await;
    for _ in 0..5 {
        node.create("q", "t").await;
        tokio::time::advance(std::time::Duration::from_millis(2)).await;
    }
    let p1 = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks?limit=2&offset=0"))
            .await
            .unwrap(),
    )
    .await;
    let p2 = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks?limit=2&offset=2"))
            .await
            .unwrap(),
    )
    .await;
    let p3 = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks?limit=2&offset=4"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(p1.as_array().unwrap().len(), 2);
    assert_eq!(p2.as_array().unwrap().len(), 2);
    assert_eq!(p3.as_array().unwrap().len(), 1);
    assert_ne!(p1[0]["id"], p2[0]["id"]);
    // newest first
    assert!(p1[0]["created_at"].as_str().unwrap() >= p1[1]["created_at"].as_str().unwrap());
}

#[tokio::test(start_paused = true)]
async fn test_rest_list_tasks_empty() {
    let node = TestNode::new().await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test(start_paused = true)]
async fn test_rest_list_tasks_combined_filters() {
    let node = TestNode::new().await;
    let a = node.create("a", "t").await;
    node.create("a", "t").await;
    let b = node.create("b", "t").await;
    node.complete(&a.id).await;
    node.complete(&b.id).await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks?queue_name=a&status=COMPLETED"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], a.id);
}

// ─── POST /api/v1/tasks/:id/cancel ──────────────────────────────────

#[tokio::test(start_paused = true)]
async fn test_rest_cancel_task_pending() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let resp = node
        .router()
        .oneshot(post_empty(&format!("/api/v1/tasks/{}/cancel", t.id)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(parse_response_json(resp).await["status"], "CANCELLED");
    assert_eq!(node.engine.pending_count("q"), 0);
}

#[tokio::test(start_paused = true)]
async fn test_rest_cancel_task_not_found() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_empty("/api/v1/tasks/nope/cancel"))
        .await
        .unwrap();
    assert_error_response(
        resp,
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_STATE",
        "not found or not in cancellable state",
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn test_rest_cancel_task_already_completed() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    node.complete(&t.id).await;
    let resp = node
        .router()
        .oneshot(post_empty(&format!("/api/v1/tasks/{}/cancel", t.id)))
        .await
        .unwrap();
    assert_error_response(
        resp,
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_STATE",
        "cancellable",
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn test_rest_cancel_running_task() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let (wid, mut rx) = node.register_worker(&["q"], 1).await;
    node.engine.dispatch(&t.id, &wid.0).unwrap();
    node.dispatcher
        .workers()
        .get_mut(wid.as_ref())
        .unwrap()
        .assign_task(t.id.clone());
    let resp = node
        .router()
        .oneshot(post_empty(&format!("/api/v1/tasks/{}/cancel", t.id)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(parse_response_json(resp).await["status"], "CANCELLED");
    // The worker was told.
    let msg = rx.recv().await.unwrap();
    assert!(
        matches!(msg.response, Some(valka_proto::worker_response::Response::TaskCancellation(c)) if c.task_id == t.id)
    );
    // The run is closed.
    let runs = node.engine.runs_for_task(&t.id).unwrap();
    assert_eq!(runs[0].status, "FAILED");
}

// ─── runs & logs ────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn test_rest_get_task_runs() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, "worker-1");
    let body = parse_response_json(
        node.router()
            .oneshot(get_req(&format!("/api/v1/tasks/{}/runs", t.id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], run);
    assert_eq!(body[0]["worker_id"], "worker-1");
    assert_eq!(body[0]["attempt_number"], 1);
    assert_eq!(body[0]["status"], "RUNNING");
    assert_eq!(body[0]["assigned_node_id"], "test-node");
}

#[tokio::test(start_paused = true)]
async fn test_rest_get_task_runs_empty() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req(&format!("/api/v1/tasks/{}/runs", t.id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body, serde_json::json!([]));
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks/unknown/runs"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test(start_paused = true)]
async fn test_rest_get_task_checkpoints() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let uri = format!("/api/v1/tasks/{}/checkpoints", t.id);
    let body = parse_response_json(node.router().oneshot(get_req(&uri)).await.unwrap()).await;
    assert_eq!(body, serde_json::json!([]));

    let run = node.start(&t.id, "worker-1");
    for (step, out) in [
        ("fetch", serde_json::json!({"rows": 2})),
        ("parse", serde_json::json!(null)),
    ] {
        node.engine
            .checkpoint(&t.id, &run, step, out)
            .await
            .unwrap();
    }
    let body = parse_response_json(node.router().oneshot(get_req(&uri)).await.unwrap()).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["step"], "fetch");
    assert_eq!(arr[0]["output"], serde_json::json!({"rows": 2}));
    assert_eq!(arr[0]["run_id"], run);
    assert_eq!(arr[0]["attempt_number"], 1);
    assert_eq!(arr[0]["task_id"], t.id);
    assert!(arr[0]["created_at"].is_string());
    assert_eq!(arr[1]["step"], "parse");
    assert!(arr[1]["output"].is_null());
}

#[tokio::test(start_paused = true)]
async fn test_rest_get_task_checkpoints_unknown_task() {
    let node = TestNode::new().await;
    let response = node
        .router()
        .oneshot(get_req("/api/v1/tasks/unknown/checkpoints"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

async fn push_logs(node: &TestNode, run: &str, n: i64) {
    for i in 0..n {
        node.log_tx
            .send(valka_wal::logstore::LogLine {
                task_run_id: run.to_string(),
                timestamp_ms: 1000 + i,
                level: "INFO".into(),
                message: format!("line {i}"),
                metadata: None,
            })
            .await
            .unwrap();
    }
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn test_rest_get_run_logs() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, "w");
    push_logs(&node, &run, 3).await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req(&format!(
                "/api/v1/tasks/{}/runs/{}/logs",
                t.id, run
            )))
            .await
            .unwrap(),
    )
    .await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["message"], "line 0");
    assert_eq!(arr[0]["id"], 1);
    assert_eq!(arr[2]["timestamp_ms"], 1002);
}

#[tokio::test(start_paused = true)]
async fn test_rest_get_run_logs_with_after_id() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, "w");
    push_logs(&node, &run, 5).await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req(&format!(
                "/api/v1/tasks/{}/runs/{}/logs?after_id=2&limit=2",
                t.id, run
            )))
            .await
            .unwrap(),
    )
    .await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["id"], 3);
    assert_eq!(arr[0]["message"], "line 2");
}

#[tokio::test(start_paused = true)]
async fn test_rest_get_run_logs_empty() {
    let node = TestNode::new().await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks/x/runs/y/logs"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body, serde_json::json!([]));
}

// ─── workers, dead letters, health ──────────────────────────────────

#[tokio::test(start_paused = true)]
async fn test_rest_list_workers_empty() {
    let node = TestNode::new().await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/workers"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test(start_paused = true)]
async fn test_rest_list_workers_connected() {
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 3).await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/workers"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], wid.0);
    assert_eq!(body[0]["concurrency"], 3);
    assert_eq!(body[0]["status"], "CONNECTED");
}

#[tokio::test(start_paused = true)]
async fn test_rest_list_dead_letters() {
    let node = TestNode::new().await;
    let t = node
        .create_with(valka_engine::CreateTask {
            max_retries: 1,
            ..task_req("q", "t")
        })
        .await;
    node.dead_letter(&t.id).await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/dead-letters"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["task_id"], t.id);
    assert_eq!(body[0]["queue_name"], "q");
    assert_eq!(body[0]["error_message"], "boom");
    assert_eq!(body[0]["attempt_count"], 1);
}

#[tokio::test(start_paused = true)]
async fn test_rest_list_dead_letters_filter_queue() {
    let node = TestNode::new().await;
    let a = node
        .create_with(valka_engine::CreateTask {
            max_retries: 1,
            ..task_req("a", "t")
        })
        .await;
    let b = node
        .create_with(valka_engine::CreateTask {
            max_retries: 1,
            ..task_req("b", "t")
        })
        .await;
    node.dead_letter(&a.id).await;
    node.dead_letter(&b.id).await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/dead-letters?queue_name=b"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["task_id"], b.id);
}

#[tokio::test(start_paused = true)]
async fn test_rest_healthz_and_cluster() {
    let node = TestNode::new().await;
    let resp = node.router().oneshot(get_req("/healthz")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/cluster"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["this_node"], "test-node");
    assert_eq!(body["clustered"], false);
    assert_eq!(body["health"]["status"], "ok");
}

// ─── signals ────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn test_rest_send_signal() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let resp = node
        .router()
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", t.id),
            serde_json::json!({"signal_name": "pause"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = parse_response_json(resp).await;
    assert!(!body["signal_id"].as_str().unwrap().is_empty());
    assert_eq!(body["delivered"], false, "no worker is running it");
}

#[tokio::test(start_paused = true)]
async fn test_rest_send_signal_with_payload() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let resp = node
        .router()
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", t.id),
            serde_json::json!({"signal_name": "cfg", "payload": {"rate": 5}}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let list = parse_response_json(
        node.router()
            .oneshot(get_req(&format!("/api/v1/tasks/{}/signals", t.id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list[0]["payload"]["rate"], 5);
    assert_eq!(list[0]["status"], "PENDING");
}

#[tokio::test(start_paused = true)]
async fn test_rest_send_signal_task_not_found() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(post_json(
            "/api/v1/tasks/00000000-0000-7000-8000-000000000001/signal",
            serde_json::json!({"signal_name": "x"}),
        ))
        .await
        .unwrap();
    assert_error_response(resp, StatusCode::NOT_FOUND, "NOT_FOUND", "not found").await;
}

async fn assert_signal_rejected(node: &TestNode, task_id: &str, status: &str) {
    let resp = node
        .router()
        .oneshot(post_json(
            &format!("/api/v1/tasks/{task_id}/signal"),
            serde_json::json!({"signal_name": "x"}),
        ))
        .await
        .unwrap();
    assert_error_response(
        resp,
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_STATE",
        status,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn test_rest_send_signal_terminal_tasks_rejected() {
    let node = TestNode::new().await;
    let c = node.create("q", "t").await;
    node.complete(&c.id).await;
    assert_signal_rejected(&node, &c.id, "COMPLETED").await;

    let f = node.create("q", "t").await;
    node.fail_terminal(&f.id).await;
    assert_signal_rejected(&node, &f.id, "FAILED").await;

    let x = node.create("q", "t").await;
    node.engine.cancel_task(&x.id, "u").await.unwrap();
    assert_signal_rejected(&node, &x.id, "CANCELLED").await;

    let d = node
        .create_with(valka_engine::CreateTask {
            max_retries: 1,
            ..task_req("q", "t")
        })
        .await;
    node.dead_letter(&d.id).await;
    assert_signal_rejected(&node, &d.id, "DEAD_LETTER").await;
}

#[tokio::test(start_paused = true)]
async fn test_rest_send_signal_running_task_delivers() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let (wid, mut rx) = node.register_worker(&["q"], 1).await;
    node.engine.dispatch(&t.id, &wid.0).unwrap();
    node.dispatcher
        .workers()
        .get_mut(wid.as_ref())
        .unwrap()
        .assign_task(t.id.clone());
    let resp = node
        .router()
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", t.id),
            serde_json::json!({"signal_name": "progress"}),
        ))
        .await
        .unwrap();
    let body = parse_response_json(resp).await;
    assert_eq!(body["delivered"], true);
    let msg = rx.recv().await.unwrap();
    assert!(
        matches!(msg.response, Some(valka_proto::worker_response::Response::TaskSignal(s)) if s.signal_name == "progress")
    );
    settle().await;
    let list = parse_response_json(
        node.router()
            .oneshot(get_req(&format!("/api/v1/tasks/{}/signals", t.id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list[0]["status"], "DELIVERED");
}

#[tokio::test(start_paused = true)]
async fn test_rest_send_signal_retry_task_allowed() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, "w");
    node.engine.fail_run(&t.id, &run, "e", true).await.unwrap();
    assert_eq!(
        node.engine.get_task(&t.id).unwrap().status,
        TaskStatus::Retry
    );
    let resp = node
        .router()
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", t.id),
            serde_json::json!({"signal_name": "x"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test(start_paused = true)]
async fn test_rest_list_signals_filter_status_and_empty() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let empty = parse_response_json(
        node.router()
            .oneshot(get_req(&format!("/api/v1/tasks/{}/signals", t.id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(empty, serde_json::json!([]));
    let s1 = node.engine.send_signal(&t.id, "a", None).await.unwrap();
    node.engine.send_signal(&t.id, "a", None).await.unwrap();
    node.engine.signal_delivered(&s1.id);
    settle().await;
    let pending = parse_response_json(
        node.router()
            .oneshot(get_req(&format!(
                "/api/v1/tasks/{}/signals?status=PENDING",
                t.id
            )))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(pending.as_array().unwrap().len(), 1);
    let all = parse_response_json(
        node.router()
            .oneshot(get_req(&format!("/api/v1/tasks/{}/signals", t.id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    assert_eq!(all[0]["signal_name"], "a");
    let bad = node
        .router()
        .oneshot(get_req(&format!(
            "/api/v1/tasks/{}/signals?status=NOPE",
            t.id
        )))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
}

// ─── DELETE ─────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn test_rest_delete_task_not_found() {
    let node = TestNode::new().await;
    let resp = node
        .router()
        .oneshot(delete_req(
            "/api/v1/tasks/00000000-0000-7000-8000-000000000001",
        ))
        .await
        .unwrap();
    assert_error_response(resp, StatusCode::NOT_FOUND, "NOT_FOUND", "not found").await;
}

#[tokio::test(start_paused = true)]
async fn test_rest_delete_task_success() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let resp = node
        .router()
        .oneshot(delete_req(&format!("/api/v1/tasks/{}", t.id)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(parse_response_json(resp).await["deleted"], true);
    let resp = node
        .router()
        .oneshot(get_req(&format!("/api/v1/tasks/{}", t.id)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(start_paused = true)]
async fn test_rest_delete_running_task_removes_runs() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    node.start(&t.id, "w");
    let resp = node
        .router()
        .oneshot(delete_req(&format!("/api/v1/tasks/{}", t.id)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(node.engine.runs_for_task(&t.id).is_none());
}

#[tokio::test(start_paused = true)]
async fn test_rest_clear_all_tasks() {
    let node = TestNode::new().await;
    for _ in 0..4 {
        node.create("q", "t").await;
    }
    let resp = node
        .router()
        .oneshot(delete_req("/api/v1/tasks"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(parse_response_json(resp).await["deleted_count"], 4);
    let body = parse_response_json(
        node.router()
            .oneshot(get_req("/api/v1/tasks"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body, serde_json::json!([]));
}

// ─── the REST layer's view survives a node restart ──────────────────

#[tokio::test(start_paused = true)]
async fn test_rest_state_survives_restart() {
    let store = valka_wal::Store::memory();
    let (a, b);
    {
        let node = TestNode::on_store(store.clone(), "n1").await;
        a = node.create("q", "t").await;
        b = node.create("q", "t").await;
        node.complete(&a.id).await;
        node.engine.send_signal(&b.id, "s", None).await.unwrap();
        node.engine.sync().await.unwrap();
    }
    let node = TestNode::on_store(store, "n1").await;
    let body = parse_response_json(
        node.router()
            .oneshot(get_req(&format!("/api/v1/tasks/{}", a.id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["status"], "COMPLETED");
    let list = parse_response_json(
        node.router()
            .oneshot(get_req(&format!("/api/v1/tasks/{}/signals", b.id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}
