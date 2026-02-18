use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use tower::ServiceExt;

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

fn get_req(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

// ─── POST /api/v1/tasks ─────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_create_task(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({
                "queue_name": "demo",
                "task_name": "email.send",
                "input": {"to": "user@example.com"}
            }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = parse_response_json(resp).await;
    assert_eq!(body["queue_name"], "demo");
    assert_eq!(body["task_name"], "email.send");
    assert_eq!(body["status"], "PENDING");
    assert!(!body["id"].as_str().unwrap().is_empty());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_create_task_minimal(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({
                "queue_name": "q",
                "task_name": "t"
            }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = parse_response_json(resp).await;
    assert_eq!(body["status"], "PENDING");
    assert!(body["input"].is_null());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_create_task_all_fields(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({
                "queue_name": "billing",
                "task_name": "charge",
                "input": {"amount": 100},
                "priority": 10,
                "max_retries": 5,
                "timeout_seconds": 600,
                "idempotency_key": "idem-001",
                "metadata": {"source": "api"}
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

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_create_task_with_scheduled_at(pool: PgPool) {
    let app = build_test_router(pool);
    let future = (Utc::now() + Duration::hours(1)).to_rfc3339();

    let resp = app
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({
                "queue_name": "q",
                "task_name": "t",
                "scheduled_at": future
            }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = parse_response_json(resp).await;
    assert!(!body["scheduled_at"].is_null());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_create_task_defaults(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/tasks",
            serde_json::json!({
                "queue_name": "q",
                "task_name": "t"
            }),
        ))
        .await
        .unwrap();

    let body = parse_response_json(resp).await;
    assert_eq!(body["priority"], 0);
    assert_eq!(body["max_retries"], 3);
    assert_eq!(body["timeout_seconds"], 300);
}

// ─── GET /api/v1/tasks/{id} ─────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_get_task(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req(&format!("/api/v1/tasks/{}", task.id)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body["id"], task.id);
    assert_eq!(body["queue_name"], "q");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_get_task_not_found(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req("/api/v1/tasks/nonexistent-id"))
        .await
        .unwrap();

    assert_error_response(resp, StatusCode::NOT_FOUND, "NOT_FOUND", "Task not found").await;
}

// ─── GET /api/v1/tasks ──────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_tasks(pool: PgPool) {
    for i in 0..5 {
        create_test_task(&pool, "q", &format!("t{i}")).await;
    }
    let app = build_test_router(pool);

    let resp = app.oneshot(get_req("/api/v1/tasks")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 5);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_tasks_filter_queue(pool: PgPool) {
    create_test_task(&pool, "demo", "t1").await;
    create_test_task(&pool, "demo", "t2").await;
    create_test_task(&pool, "other", "t3").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req("/api/v1/tasks?queue_name=demo"))
        .await
        .unwrap();

    let body = parse_response_json(resp).await;
    let tasks = body.as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|t| t["queue_name"] == "demo"));
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_tasks_filter_status(pool: PgPool) {
    let t1 = create_test_task(&pool, "q", "t1").await;
    create_test_task(&pool, "q", "t2").await;
    valka_db::queries::tasks::complete_task(&pool, &t1.id, None)
        .await
        .unwrap();
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req("/api/v1/tasks?status=PENDING"))
        .await
        .unwrap();

    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_tasks_pagination(pool: PgPool) {
    for i in 0..5 {
        create_test_task(&pool, "q", &format!("t{i}")).await;
    }
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req("/api/v1/tasks?limit=2&offset=2"))
        .await
        .unwrap();

    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 2);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_tasks_empty(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app.oneshot(get_req("/api/v1/tasks")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

// ─── POST /api/v1/tasks/{id}/cancel ─────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_cancel_task_pending(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/cancel", task.id),
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body["status"], "CANCELLED");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_cancel_task_not_found(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/tasks/nonexistent/cancel",
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    assert_error_response(
        resp,
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_STATE",
        "not in cancellable state",
    )
    .await;
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_cancel_task_already_completed(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    valka_db::queries::tasks::complete_task(&pool, &task.id, None)
        .await
        .unwrap();
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/cancel", task.id),
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    assert_error_response(
        resp,
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_STATE",
        "not in cancellable state",
    )
    .await;
}

// ─── GET /api/v1/tasks/{id}/runs ────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_get_task_runs(pool: PgPool) {
    let (task, _run) = create_running_task(&pool, "q").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req(&format!("/api/v1/tasks/{}/runs", task.id)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    let runs = body.as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["task_id"], task.id);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_get_task_runs_empty(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req(&format!("/api/v1/tasks/{}/runs", task.id)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

// ─── GET /api/v1/tasks/{task_id}/runs/{run_id}/logs ─────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_get_run_logs(pool: PgPool) {
    let (task, run) = create_running_task(&pool, "q").await;
    let entries = vec![valka_db::queries::task_logs::InsertLogEntry {
        task_run_id: run.id.clone(),
        timestamp_ms: 1000,
        level: "INFO".to_string(),
        message: "hello".to_string(),
        metadata: None,
    }];
    valka_db::queries::task_logs::batch_insert_logs(&pool, &entries)
        .await
        .unwrap();
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req(&format!(
            "/api/v1/tasks/{}/runs/{}/logs",
            task.id, run.id
        )))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    let logs = body.as_array().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["message"], "hello");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_get_run_logs_with_after_id(pool: PgPool) {
    let (task, run) = create_running_task(&pool, "q").await;
    let entries: Vec<valka_db::queries::task_logs::InsertLogEntry> = (0..5)
        .map(|i| valka_db::queries::task_logs::InsertLogEntry {
            task_run_id: run.id.clone(),
            timestamp_ms: 1000 + i,
            level: "INFO".to_string(),
            message: format!("msg-{i}"),
            metadata: None,
        })
        .collect();
    valka_db::queries::task_logs::batch_insert_logs(&pool, &entries)
        .await
        .unwrap();

    // Get all logs to find an ID for cursor
    let app = build_test_router(pool.clone());
    let resp = app
        .oneshot(get_req(&format!(
            "/api/v1/tasks/{}/runs/{}/logs?limit=2",
            task.id, run.id
        )))
        .await
        .unwrap();
    let body = parse_response_json(resp).await;
    let logs = body.as_array().unwrap();
    let after_id = logs.last().unwrap()["id"].as_i64().unwrap();

    // Page 2 using after_id
    let app2 = build_test_router(pool);
    let resp2 = app2
        .oneshot(get_req(&format!(
            "/api/v1/tasks/{}/runs/{}/logs?limit=2&after_id={after_id}",
            task.id, run.id
        )))
        .await
        .unwrap();
    let body2 = parse_response_json(resp2).await;
    let page2 = body2.as_array().unwrap();
    assert_eq!(page2.len(), 2);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_get_run_logs_empty(pool: PgPool) {
    let (task, run) = create_running_task(&pool, "q").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req(&format!(
            "/api/v1/tasks/{}/runs/{}/logs",
            task.id, run.id
        )))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

// ─── GET /api/v1/workers ────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_workers_empty(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app.oneshot(get_req("/api/v1/workers")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

// ─── GET /api/v1/dead-letters ───────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_dead_letters(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    valka_db::queries::dead_letter::insert_dead_letter(
        &pool,
        &uuid::Uuid::now_v7().to_string(),
        &task.id,
        "q",
        "t",
        None,
        Some("error"),
        3,
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let app = build_test_router(pool);

    let resp = app.oneshot(get_req("/api/v1/dead-letters")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["task_id"], task.id);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_dead_letters_filter_queue(pool: PgPool) {
    let task_a = create_test_task(&pool, "queue-a", "t").await;
    let task_b = create_test_task(&pool, "queue-b", "t").await;

    for (task, queue) in [(&task_a, "queue-a"), (&task_b, "queue-b")] {
        valka_db::queries::dead_letter::insert_dead_letter(
            &pool,
            &uuid::Uuid::now_v7().to_string(),
            &task.id,
            queue,
            "t",
            None,
            None,
            1,
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req("/api/v1/dead-letters?queue_name=queue-a"))
        .await
        .unwrap();

    let body = parse_response_json(resp).await;
    let dls = body.as_array().unwrap();
    assert_eq!(dls.len(), 1);
    assert_eq!(dls[0]["queue_name"], "queue-a");
}

// ─── GET /healthz ───────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_healthz(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app.oneshot(get_req("/healthz")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"ok");
}

// ─── POST /api/v1/tasks/{id}/signal ─────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_signal(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", task.id),
            serde_json::json!({ "signal_name": "approve" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = parse_response_json(resp).await;
    assert!(!body["signal_id"].as_str().unwrap().is_empty());
    // No worker connected in tests, so delivered should be false
    assert_eq!(body["delivered"], false);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_signal_with_payload(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let app = build_test_router(pool.clone());

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", task.id),
            serde_json::json!({
                "signal_name": "data",
                "payload": {"key": "value", "count": 42}
            }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);

    // Verify payload persisted via list
    let app2 = build_test_router(pool);
    let resp2 = app2
        .oneshot(get_req(&format!("/api/v1/tasks/{}/signals", task.id)))
        .await
        .unwrap();
    let body = parse_response_json(resp2).await;
    let signals = body.as_array().unwrap();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0]["payload"]["key"], "value");
    assert_eq!(signals[0]["payload"]["count"], 42);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_signal_no_payload(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", task.id),
            serde_json::json!({ "signal_name": "ping" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_signal_task_not_found(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/tasks/nonexistent-id/signal",
            serde_json::json!({ "signal_name": "test" }),
        ))
        .await
        .unwrap();

    assert_error_response(resp, StatusCode::NOT_FOUND, "NOT_FOUND", "Task not found").await;
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_signal_completed_task(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    valka_db::queries::tasks::complete_task(&pool, &task.id, None)
        .await
        .unwrap();
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", task.id),
            serde_json::json!({ "signal_name": "test" }),
        ))
        .await
        .unwrap();

    assert_error_response(
        resp,
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_STATE",
        "Cannot send signal to task in COMPLETED state",
    )
    .await;
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_signal_failed_task(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    valka_db::queries::tasks::fail_task(&pool, &task.id, "error")
        .await
        .unwrap();
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", task.id),
            serde_json::json!({ "signal_name": "test" }),
        ))
        .await
        .unwrap();

    assert_error_response(
        resp,
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_STATE",
        "Cannot send signal to task in FAILED state",
    )
    .await;
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_signal_cancelled_task(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    valka_db::queries::tasks::cancel_task_any(&pool, &task.id)
        .await
        .unwrap();
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", task.id),
            serde_json::json!({ "signal_name": "test" }),
        ))
        .await
        .unwrap();

    assert_error_response(
        resp,
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_STATE",
        "Cannot send signal to task in CANCELLED state",
    )
    .await;
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_signal_running_task(pool: PgPool) {
    let (task, _run) = create_running_task(&pool, "q").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", task.id),
            serde_json::json!({ "signal_name": "pause" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_signal_retry_task(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    valka_db::queries::tasks::update_task_status(&pool, &task.id, "RETRY")
        .await
        .unwrap();
    let app = build_test_router(pool);

    let resp = app
        .oneshot(post_json(
            &format!("/api/v1/tasks/{}/signal", task.id),
            serde_json::json!({ "signal_name": "nudge" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
}

// ─── GET /api/v1/tasks/{id}/signals ─────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_signals(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    for i in 0..3 {
        valka_db::queries::signals::create_signal(
            &pool,
            &format!("sig-{i}"),
            &task.id,
            &format!("signal-{i}"),
            None,
        )
        .await
        .unwrap();
    }
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req(&format!("/api/v1/tasks/{}/signals", task.id)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 3);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_signals_filter_status(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    valka_db::queries::signals::create_signal(&pool, "s1", &task.id, "a", None)
        .await
        .unwrap();
    valka_db::queries::signals::create_signal(&pool, "s2", &task.id, "b", None)
        .await
        .unwrap();
    valka_db::queries::signals::mark_delivered(&pool, "s2")
        .await
        .unwrap();
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req(&format!(
            "/api/v1/tasks/{}/signals?status=PENDING",
            task.id
        )))
        .await
        .unwrap();

    let body = parse_response_json(resp).await;
    let signals = body.as_array().unwrap();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0]["status"], "PENDING");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_list_signals_empty(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(get_req(&format!("/api/v1/tasks/{}/signals", task.id)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_send_multiple_signals_same_name(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let app = build_test_router(pool.clone());

    for _ in 0..3 {
        let app_inner = build_test_router(pool.clone());
        let resp = app_inner
            .oneshot(post_json(
                &format!("/api/v1/tasks/{}/signal", task.id),
                serde_json::json!({ "signal_name": "approve" }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // Verify all 3 persisted
    let resp = app
        .oneshot(get_req(&format!("/api/v1/tasks/{}/signals", task.id)))
        .await
        .unwrap();
    let body = parse_response_json(resp).await;
    let signals = body.as_array().unwrap();
    assert_eq!(signals.len(), 3);
    assert!(signals.iter().all(|s| s["signal_name"] == "approve"));
}

// ─── DELETE /api/v1/tasks/{id} error ─────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_delete_task_not_found(pool: PgPool) {
    let app = build_test_router(pool);

    let resp = app
        .oneshot(delete_req("/api/v1/tasks/nonexistent-id"))
        .await
        .unwrap();

    assert_error_response(resp, StatusCode::NOT_FOUND, "NOT_FOUND", "Task not found").await;
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_rest_delete_task_success(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let app = build_test_router(pool);

    let resp = app
        .oneshot(delete_req(&format!("/api/v1/tasks/{}", task.id)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_response_json(resp).await;
    assert_eq!(body["deleted"], true);
}
