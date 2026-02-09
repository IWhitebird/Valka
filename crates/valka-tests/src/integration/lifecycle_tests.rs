use chrono::{Duration, Utc};
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use valka_core::{MatchingConfig, NodeId, WorkerId};
use valka_db::queries::{dead_letter, task_runs, tasks};
use valka_dispatcher::DispatcherService;
use valka_dispatcher::worker_handle::WorkerHandle;
use valka_matching::MatchingService;
use valka_proto::WorkerResponse;

use super::helpers::*;

async fn setup(
    pool: PgPool,
) -> (
    DispatcherService,
    MatchingService,
    mpsc::Receiver<WorkerResponse>,
) {
    let config = MatchingConfig::default();
    let matching = MatchingService::new(config);
    let node_id = NodeId::new();
    let (event_tx, _) = broadcast::channel::<valka_proto::TaskEvent>(128);
    let (log_tx, _) = mpsc::channel::<valka_proto::LogEntry>(128);

    let dispatcher = DispatcherService::new(matching.clone(), pool, node_id, event_tx, log_tx);

    // Register a worker
    let (tx, rx) = mpsc::channel::<WorkerResponse>(16);
    let handle = WorkerHandle::new(
        WorkerId::new(),
        "lifecycle-worker".to_string(),
        vec!["lifecycle".to_string()],
        4,
        tx,
        String::new(),
    );
    dispatcher.register_worker(handle).await;

    (dispatcher, matching, rx)
}

// ─── Happy Path ─────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_task_create_dispatch_complete(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let (dispatcher, _matching, _rx) = setup(pool.clone()).await;

    // Simulate dispatch: create run, set RUNNING
    let run_id = uuid::Uuid::now_v7().to_string();
    tasks::update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();
    tasks::increment_attempt_count(&pool, &task.id)
        .await
        .unwrap();
    let run = task_runs::create_task_run(
        &pool,
        task_runs::CreateTaskRunParams {
            id: run_id.clone(),
            task_id: task.id.clone(),
            attempt_number: 1,
            worker_id: uuid::Uuid::now_v7().to_string(),
            assigned_node_id: uuid::Uuid::now_v7().to_string(),
            lease_expires_at: Utc::now() + Duration::seconds(330),
        },
    )
    .await
    .unwrap();

    // Get any worker_id from dispatcher
    let worker_id = {
        let entry = dispatcher.workers().iter().next().unwrap();
        WorkerId(entry.key().clone())
    };

    // Handle success
    let result = valka_proto::TaskResult {
        task_id: task.id.clone(),
        task_run_id: run.id.clone(),
        success: true,
        output: serde_json::json!({"processed": true}).to_string(),
        error_message: String::new(),
        retryable: false,
    };
    dispatcher.handle_task_result(&worker_id, result).await;

    // Verify final state
    let final_task = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(final_task.status, "COMPLETED");
    assert_eq!(final_task.output.unwrap()["processed"], true);
    assert_eq!(final_task.attempt_count, 1);

    let final_run = task_runs::get_task_run(&pool, &run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_run.status, "COMPLETED");
    assert!(final_run.completed_at.is_some());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_task_with_output_preserved(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let output = serde_json::json!({"data": [1, 2, 3], "meta": {"count": 3}});

    tasks::update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();
    tasks::increment_attempt_count(&pool, &task.id)
        .await
        .unwrap();
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;

    // Complete with output
    task_runs::complete_task_run(&pool, &run.id, Some(output.clone()))
        .await
        .unwrap();
    tasks::complete_task(&pool, &task.id, Some(output.clone()))
        .await
        .unwrap();

    // Verify output preserved on both
    let final_task = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(final_task.output.unwrap(), output);

    let final_run = task_runs::get_task_run(&pool, &run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_run.output.unwrap(), output);
}

// ─── Retry Flow ─────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_task_fail_retry_succeed(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    // Attempt 1: dispatch and fail
    tasks::update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();
    tasks::increment_attempt_count(&pool, &task.id)
        .await
        .unwrap();
    let run1 = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;
    task_runs::fail_task_run(&pool, &run1.id, "timeout")
        .await
        .unwrap();
    tasks::update_task_status(&pool, &task.id, "RETRY")
        .await
        .unwrap();

    // Process retries → sets scheduled_at
    valka_scheduler::retry::process_retries(&pool, 1, 3600)
        .await
        .unwrap();
    let retrying = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert!(retrying.scheduled_at.is_some());

    // Fast-forward: set scheduled_at to the past so promote picks it up
    sqlx::query("UPDATE tasks SET scheduled_at = NOW() - INTERVAL '1 second' WHERE id = $1")
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();

    // Promote → PENDING
    valka_scheduler::delayed::promote_delayed_tasks(&pool)
        .await
        .unwrap();
    let pending = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(pending.status, "PENDING");
    assert!(pending.scheduled_at.is_none());

    // Attempt 2: dispatch and succeed
    tasks::update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();
    tasks::increment_attempt_count(&pool, &task.id)
        .await
        .unwrap();
    let run2 = create_test_run(&pool, &task.id, 2, Utc::now() + Duration::seconds(300)).await;
    task_runs::complete_task_run(&pool, &run2.id, Some(serde_json::json!({"ok": true})))
        .await
        .unwrap();
    tasks::complete_task(&pool, &task.id, Some(serde_json::json!({"ok": true})))
        .await
        .unwrap();

    let final_task = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(final_task.status, "COMPLETED");
    assert_eq!(final_task.attempt_count, 2);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_task_exhaust_retries_to_dlq(pool: PgPool) {
    let mut params = default_task_params("q", "t");
    params.max_retries = 2;
    let task = create_test_task_full(&pool, params).await;

    // Exhaust all retries
    for attempt in 1..=2 {
        tasks::update_task_status(&pool, &task.id, "RUNNING")
            .await
            .unwrap();
        tasks::increment_attempt_count(&pool, &task.id)
            .await
            .unwrap();
        let run = create_test_run(
            &pool,
            &task.id,
            attempt,
            Utc::now() + Duration::seconds(300),
        )
        .await;
        task_runs::fail_task_run(&pool, &run.id, "error")
            .await
            .unwrap();
    }

    // Final failure: set to FAILED with attempt_count >= max_retries
    tasks::fail_task(&pool, &task.id, "final error")
        .await
        .unwrap();

    // Process DLQ
    let count = valka_scheduler::dlq::process_dead_letters(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let final_task = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(final_task.status, "DEAD_LETTER");

    let dls = dead_letter::list_dead_letters(&pool, None, 50, 0)
        .await
        .unwrap();
    assert_eq!(dls.len(), 1);
    assert_eq!(dls[0].task_id, task.id);
}

// ─── Cancellation ───────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_cancel_pending_task(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    let cancelled = tasks::cancel_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(cancelled.status, "CANCELLED");

    // Should NOT be dequeue-able
    let dequeued = tasks::dequeue_tasks(&pool, "q", task.partition_id, 10)
        .await
        .unwrap();
    assert!(
        dequeued.iter().all(|t| t.id != task.id),
        "Cancelled task should not be dequeued"
    );
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_cancel_running_task_via_dispatcher(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    tasks::update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();

    // Set up dispatcher with a worker holding this task
    let matching = MatchingService::new(MatchingConfig::default());
    let (event_tx, _) = broadcast::channel::<valka_proto::TaskEvent>(128);
    let (log_tx, _) = mpsc::channel::<valka_proto::LogEntry>(128);
    let dispatcher =
        DispatcherService::new(matching, pool.clone(), NodeId::new(), event_tx, log_tx);

    let (tx, mut rx) = mpsc::channel::<WorkerResponse>(16);
    let handle = WorkerHandle::new(
        WorkerId::new(),
        "w1".to_string(),
        vec!["q".to_string()],
        2,
        tx,
        String::new(),
    );
    let worker_id = handle.worker_id.clone();
    dispatcher.register_worker(handle).await;

    // Assign task to worker
    if let Some(mut h) = dispatcher.workers().get_mut(worker_id.as_ref()) {
        h.assign_task(task.id.clone());
    }

    // Cancel via dispatcher
    let sent = dispatcher.cancel_task_on_worker(&task.id).await;
    assert!(sent);

    // Verify worker received cancellation
    let msg = rx.recv().await.unwrap();
    match msg.response.unwrap() {
        valka_proto::worker_response::Response::TaskCancellation(c) => {
            assert_eq!(c.task_id, task.id);
        }
        _ => panic!("Expected TaskCancellation"),
    }

    // Cancel in DB
    tasks::cancel_task_any(&pool, &task.id).await.unwrap();
    let final_task = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(final_task.status, "CANCELLED");
}

// ─── Cold Path ──────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dequeue_dispatching_flow(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    assert_eq!(task.status, "PENDING");

    // Cold path: dequeue
    let dequeued = tasks::dequeue_tasks(&pool, "q", task.partition_id, 10)
        .await
        .unwrap();
    assert_eq!(dequeued.len(), 1);
    assert_eq!(dequeued[0].status, "DISPATCHING");
    assert_eq!(dequeued[0].id, task.id);

    // After dequeue, task should be DISPATCHING in DB
    let after = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(after.status, "DISPATCHING");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_scheduled_task_not_dequeued_early(pool: PgPool) {
    let mut params = default_task_params("q", "delayed");
    params.partition_id = 0;
    params.scheduled_at = Some(Utc::now() + Duration::hours(1));
    let task = create_test_task_full(&pool, params).await;

    // Should NOT be dequeued (scheduled in the future)
    let dequeued = tasks::dequeue_tasks(&pool, "q", 0, 10).await.unwrap();
    assert!(dequeued.is_empty());

    // Fast-forward: move scheduled_at to the past
    sqlx::query("UPDATE tasks SET scheduled_at = NOW() - INTERVAL '1 second' WHERE id = $1")
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();

    // Now it should be dequeued
    let dequeued = tasks::dequeue_tasks(&pool, "q", 0, 10).await.unwrap();
    assert_eq!(dequeued.len(), 1);
    assert_eq!(dequeued[0].id, task.id);
}

// ─── Concurrency ────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_concurrent_dequeue_skip_locked(pool: PgPool) {
    let mut params = default_task_params("q", "only-one");
    params.partition_id = 0;
    create_test_task_full(&pool, params).await;

    // Two concurrent dequeue calls — only one should get the task
    let (r1, r2) = tokio::join!(
        tasks::dequeue_tasks(&pool, "q", 0, 1),
        tasks::dequeue_tasks(&pool, "q", 0, 1),
    );

    let total = r1.unwrap().len() + r2.unwrap().len();
    assert_eq!(total, 1, "SKIP LOCKED: exactly one dequeue should succeed");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_multiple_tasks_distributed(pool: PgPool) {
    // Create 3 tasks in the same partition
    for i in 0..3 {
        let mut params = default_task_params("q", &format!("t{i}"));
        params.partition_id = 0;
        create_test_task_full(&pool, params).await;
    }

    // Dequeue all 3
    let dequeued = tasks::dequeue_tasks(&pool, "q", 0, 10).await.unwrap();
    assert_eq!(dequeued.len(), 3);
}

// ─── Edge Cases ─────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_crash_recovery_orphaned_dispatching(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    // Simulate crash: task stuck in DISPATCHING with no run
    tasks::update_task_status(&pool, &task.id, "DISPATCHING")
        .await
        .unwrap();

    // Recovery
    let recovered = tasks::recover_orphaned_dispatching(&pool).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, task.id);

    let final_task = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(final_task.status, "PENDING");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_idempotency_key_prevents_duplicate(pool: PgPool) {
    let mut p1 = default_task_params("q", "t");
    p1.idempotency_key = Some("idem-key".to_string());
    create_test_task_full(&pool, p1).await;

    let mut p2 = default_task_params("q", "t");
    p2.idempotency_key = Some("idem-key".to_string());
    let result = tasks::create_task(&pool, p2).await;
    assert!(result.is_err(), "Duplicate idempotency key should fail");
}
