use chrono::{Duration, Utc};
use sqlx::PgPool;
use valka_core::TaskId;
use valka_db::queries::tasks::*;

use super::helpers::*;

// ─── CRUD ───────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_create_task_returns_pending(pool: PgPool) {
    let task = create_test_task(&pool, "demo", "email.send").await;

    assert_eq!(task.status, "PENDING");
    assert_eq!(task.attempt_count, 0);
    assert_eq!(task.queue_name, "demo");
    assert_eq!(task.task_name, "email.send");
    assert_eq!(task.priority, 0);
    assert_eq!(task.max_retries, 3);
    assert_eq!(task.timeout_seconds, 300);
    assert!(task.output.is_none());
    assert!(task.error_message.is_none());
    assert!(task.scheduled_at.is_none());
    assert!(!task.id.is_empty());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_create_task_with_all_fields(pool: PgPool) {
    let scheduled = Utc::now() + Duration::hours(1);
    let params = CreateTaskParams {
        id: TaskId::new().0,
        queue_name: "billing".to_string(),
        task_name: "charge.card".to_string(),
        partition_id: 2,
        input: Some(serde_json::json!({"amount": 100})),
        priority: 10,
        max_retries: 5,
        timeout_seconds: 600,
        idempotency_key: Some("idem-123".to_string()),
        metadata: serde_json::json!({"source": "api"}),
        scheduled_at: Some(scheduled),
    };
    let task = create_test_task_full(&pool, params).await;

    assert_eq!(task.queue_name, "billing");
    assert_eq!(task.task_name, "charge.card");
    assert_eq!(task.partition_id, 2);
    assert_eq!(task.priority, 10);
    assert_eq!(task.max_retries, 5);
    assert_eq!(task.timeout_seconds, 600);
    assert_eq!(task.idempotency_key.as_deref(), Some("idem-123"));
    assert_eq!(task.metadata["source"], "api");
    assert!(task.scheduled_at.is_some());
    assert_eq!(task.input.unwrap()["amount"], 100);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_create_task_duplicate_idempotency_key(pool: PgPool) {
    let mut p1 = default_task_params("q", "t");
    p1.idempotency_key = Some("unique-key".to_string());
    create_test_task_full(&pool, p1).await;

    let mut p2 = default_task_params("q", "t");
    p2.idempotency_key = Some("unique-key".to_string());
    let result = create_task(&pool, p2).await;
    assert!(result.is_err(), "Duplicate idempotency_key should fail");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_task_exists(pool: PgPool) {
    let created = create_test_task(&pool, "q", "t").await;
    let fetched = get_task(&pool, &created.id).await.unwrap().unwrap();

    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.queue_name, created.queue_name);
    assert_eq!(fetched.task_name, created.task_name);
    assert_eq!(fetched.status, created.status);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_task_not_found(pool: PgPool) {
    let result = get_task(&pool, "nonexistent-id").await.unwrap();
    assert!(result.is_none());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_tasks_empty(pool: PgPool) {
    let tasks = list_tasks(&pool, None, None, 50, 0).await.unwrap();
    assert!(tasks.is_empty());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_tasks_pagination(pool: PgPool) {
    for i in 0..10 {
        create_test_task(&pool, "q", &format!("task-{i}")).await;
    }

    let page = list_tasks(&pool, None, None, 3, 3).await.unwrap();
    assert_eq!(page.len(), 3);

    let all = list_tasks(&pool, None, None, 50, 0).await.unwrap();
    assert_eq!(all.len(), 10);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_tasks_filter_by_queue(pool: PgPool) {
    create_test_task(&pool, "queue-a", "t1").await;
    create_test_task(&pool, "queue-a", "t2").await;
    create_test_task(&pool, "queue-b", "t3").await;

    let a_tasks = list_tasks(&pool, Some("queue-a"), None, 50, 0)
        .await
        .unwrap();
    assert_eq!(a_tasks.len(), 2);
    assert!(a_tasks.iter().all(|t| t.queue_name == "queue-a"));

    let b_tasks = list_tasks(&pool, Some("queue-b"), None, 50, 0)
        .await
        .unwrap();
    assert_eq!(b_tasks.len(), 1);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_tasks_filter_by_status(pool: PgPool) {
    let t1 = create_test_task(&pool, "q", "t1").await;
    let _t2 = create_test_task(&pool, "q", "t2").await;

    // Complete t1
    complete_task(&pool, &t1.id, None).await.unwrap();

    let pending = list_tasks(&pool, None, Some("PENDING"), 50, 0)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);

    let completed = list_tasks(&pool, None, Some("COMPLETED"), 50, 0)
        .await
        .unwrap();
    assert_eq!(completed.len(), 1);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_tasks_ordered_by_created_at_desc(pool: PgPool) {
    let t1 = create_test_task(&pool, "q", "first").await;
    let t2 = create_test_task(&pool, "q", "second").await;
    let t3 = create_test_task(&pool, "q", "third").await;

    let tasks = list_tasks(&pool, None, None, 50, 0).await.unwrap();
    // newest first
    assert_eq!(tasks[0].id, t3.id);
    assert_eq!(tasks[1].id, t2.id);
    assert_eq!(tasks[2].id, t1.id);
}

// ─── State Transitions ─────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_update_task_status(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let original_updated = task.updated_at;

    let updated = update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(updated.status, "RUNNING");
    assert!(updated.updated_at >= original_updated);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_update_task_status_nonexistent(pool: PgPool) {
    let result = update_task_status(&pool, "nonexistent", "RUNNING")
        .await
        .unwrap();
    assert!(result.is_none());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_increment_attempt_count(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    assert_eq!(task.attempt_count, 0);

    let t1 = increment_attempt_count(&pool, &task.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t1.attempt_count, 1);

    let t2 = increment_attempt_count(&pool, &task.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t2.attempt_count, 2);
    assert!(t2.updated_at >= t1.updated_at);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_complete_task_with_output(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let output = serde_json::json!({"result": "success", "count": 42});

    let completed = complete_task(&pool, &task.id, Some(output.clone()))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(completed.status, "COMPLETED");
    assert_eq!(completed.output.unwrap(), output);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_complete_task_null_output(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    let completed = complete_task(&pool, &task.id, None).await.unwrap().unwrap();

    assert_eq!(completed.status, "COMPLETED");
    assert!(completed.output.is_none());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_fail_task_with_error(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    let failed = fail_task(&pool, &task.id, "Connection timeout")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(failed.status, "FAILED");
    assert_eq!(failed.error_message.as_deref(), Some("Connection timeout"));
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_cancel_task_pending(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    assert_eq!(task.status, "PENDING");

    let cancelled = cancel_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(cancelled.status, "CANCELLED");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_cancel_task_running_rejected(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();

    // cancel_task only works on PENDING/RETRY, not RUNNING
    let result = cancel_task(&pool, &task.id).await.unwrap();
    assert!(result.is_none(), "cancel_task should reject RUNNING tasks");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_cancel_task_any_running(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();

    let cancelled = cancel_task_any(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(cancelled.status, "CANCELLED");
}

// ─── Dequeue (SKIP LOCKED) ─────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dequeue_tasks_basic(pool: PgPool) {
    // Create 3 tasks in same queue/partition
    let mut params = default_task_params("dequeue-q", "t1");
    params.partition_id = 0;
    create_test_task_full(&pool, params).await;

    let mut params = default_task_params("dequeue-q", "t2");
    params.partition_id = 0;
    create_test_task_full(&pool, params).await;

    let mut params = default_task_params("dequeue-q", "t3");
    params.partition_id = 0;
    create_test_task_full(&pool, params).await;

    // Dequeue batch of 2
    let dequeued = dequeue_tasks(&pool, "dequeue-q", 0, 2).await.unwrap();
    assert_eq!(dequeued.len(), 2);

    // All dequeued should be DISPATCHING
    for task in &dequeued {
        assert_eq!(task.status, "DISPATCHING");
    }

    // 1 remaining PENDING
    let remaining = list_tasks(&pool, Some("dequeue-q"), Some("PENDING"), 50, 0)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dequeue_tasks_respects_priority(pool: PgPool) {
    let mut low = default_task_params("prio-q", "low");
    low.partition_id = 0;
    low.priority = 0;
    create_test_task_full(&pool, low).await;

    let mut high = default_task_params("prio-q", "high");
    high.partition_id = 0;
    high.priority = 10;
    create_test_task_full(&pool, high).await;

    let dequeued = dequeue_tasks(&pool, "prio-q", 0, 1).await.unwrap();
    assert_eq!(dequeued.len(), 1);
    assert_eq!(
        dequeued[0].task_name, "high",
        "Higher priority dequeued first"
    );
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dequeue_tasks_respects_scheduled_at(pool: PgPool) {
    // Future task — should NOT be dequeued
    let mut future = default_task_params("sched-q", "future");
    future.partition_id = 0;
    future.scheduled_at = Some(Utc::now() + Duration::hours(1));
    create_test_task_full(&pool, future).await;

    // Past task — should be dequeued
    let mut past = default_task_params("sched-q", "past");
    past.partition_id = 0;
    past.scheduled_at = Some(Utc::now() - Duration::seconds(10));
    create_test_task_full(&pool, past).await;

    // No scheduled_at — should be dequeued
    let mut none_sched = default_task_params("sched-q", "none");
    none_sched.partition_id = 0;
    create_test_task_full(&pool, none_sched).await;

    let dequeued = dequeue_tasks(&pool, "sched-q", 0, 10).await.unwrap();
    assert_eq!(
        dequeued.len(),
        2,
        "Only past and NULL scheduled_at dequeued"
    );

    let names: Vec<&str> = dequeued.iter().map(|t| t.task_name.as_str()).collect();
    assert!(!names.contains(&"future"));
}

// ─── Recovery ───────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_recover_orphaned_dispatching(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    // Set to DISPATCHING without creating a task_run
    update_task_status(&pool, &task.id, "DISPATCHING")
        .await
        .unwrap();

    let recovered = recover_orphaned_dispatching(&pool).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, task.id);
    assert_eq!(recovered[0].status, "PENDING");
}
