use chrono::{Duration, Utc};
use sqlx::PgPool;
use valka_db::queries::{dead_letter, task_runs, tasks};

use super::helpers::*;

// ─── Retry Processing ───────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_process_retries_schedules_delay(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    // Set to RETRY with no scheduled_at
    tasks::update_task_status(&pool, &task.id, "RETRY")
        .await
        .unwrap();

    let count = valka_scheduler::retry::process_retries(&pool, 1, 3600)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Verify scheduled_at is now set
    let updated = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(updated.status, "RETRY");
    assert!(updated.scheduled_at.is_some(), "scheduled_at should be set");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_process_retries_no_retry_tasks(pool: PgPool) {
    // Only PENDING tasks, no RETRY
    create_test_task(&pool, "q", "t").await;

    let count = valka_scheduler::retry::process_retries(&pool, 1, 3600)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_process_retries_skips_already_scheduled(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    // Set to RETRY WITH scheduled_at (already processed)
    tasks::schedule_retry(&pool, &task.id, Utc::now() + Duration::hours(1))
        .await
        .unwrap();

    let count = valka_scheduler::retry::process_retries(&pool, 1, 3600)
        .await
        .unwrap();
    assert_eq!(count, 0, "Already scheduled RETRY should be skipped");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_process_retries_respects_attempt_count(pool: PgPool) {
    // Task with 0 attempts
    let t1 = create_test_task(&pool, "q", "t1").await;
    tasks::update_task_status(&pool, &t1.id, "RETRY")
        .await
        .unwrap();

    // Task with 3 attempts (higher delay)
    let t2 = create_test_task(&pool, "q", "t2").await;
    tasks::increment_attempt_count(&pool, &t2.id).await.unwrap();
    tasks::increment_attempt_count(&pool, &t2.id).await.unwrap();
    tasks::increment_attempt_count(&pool, &t2.id).await.unwrap();
    tasks::update_task_status(&pool, &t2.id, "RETRY")
        .await
        .unwrap();

    valka_scheduler::retry::process_retries(&pool, 1, 3600)
        .await
        .unwrap();

    let t1_updated = tasks::get_task(&pool, &t1.id).await.unwrap().unwrap();
    let t2_updated = tasks::get_task(&pool, &t2.id).await.unwrap().unwrap();

    // t2 should have a later scheduled_at due to higher attempt count
    assert!(
        t2_updated.scheduled_at.unwrap() > t1_updated.scheduled_at.unwrap(),
        "Higher attempt count should produce later scheduled_at"
    );
}

// ─── Delayed Task Promotion ─────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_promote_delayed_tasks(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    // Set to RETRY with past scheduled_at
    tasks::schedule_retry(&pool, &task.id, Utc::now() - Duration::seconds(10))
        .await
        .unwrap();

    let count = valka_scheduler::delayed::promote_delayed_tasks(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let updated = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(updated.status, "PENDING");
    assert!(
        updated.scheduled_at.is_none(),
        "scheduled_at should be cleared"
    );
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_promote_delayed_tasks_future(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    // Set to RETRY with FUTURE scheduled_at — should NOT be promoted
    tasks::schedule_retry(&pool, &task.id, Utc::now() + Duration::hours(1))
        .await
        .unwrap();

    let count = valka_scheduler::delayed::promote_delayed_tasks(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    let unchanged = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(unchanged.status, "RETRY");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_promote_delayed_tasks_none(pool: PgPool) {
    // No RETRY tasks at all
    create_test_task(&pool, "q", "t").await;

    let count = valka_scheduler::delayed::promote_delayed_tasks(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// ─── Lease Reaping ──────────────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reap_expired_leases_retries(pool: PgPool) {
    let (task, _run) = create_running_task(&pool, "q").await;

    // Set the lease to be expired
    sqlx::query(
        "UPDATE task_runs SET lease_expires_at = NOW() - INTERVAL '1 minute' WHERE task_id = $1",
    )
    .bind(&task.id)
    .execute(&pool)
    .await
    .unwrap();

    let count = valka_scheduler::reaper::reap_expired_leases(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // task.attempt_count=0, max_retries=3 → should RETRY
    let updated = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(updated.status, "RETRY");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reap_expired_leases_dlq(pool: PgPool) {
    let (task, _run) = create_running_task(&pool, "q").await;

    // Exhaust retries: set attempt_count = max_retries
    sqlx::query("UPDATE tasks SET attempt_count = max_retries WHERE id = $1")
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();

    // Expire the lease
    sqlx::query(
        "UPDATE task_runs SET lease_expires_at = NOW() - INTERVAL '1 minute' WHERE task_id = $1",
    )
    .bind(&task.id)
    .execute(&pool)
    .await
    .unwrap();

    let count = valka_scheduler::reaper::reap_expired_leases(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Should be DEAD_LETTER
    let updated = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(updated.status, "DEAD_LETTER");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reap_expired_leases_none(pool: PgPool) {
    // No expired leases
    let (_task, _run) = create_running_task(&pool, "q").await;

    let count = valka_scheduler::reaper::reap_expired_leases(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reap_expired_leases_valid_lease_untouched(pool: PgPool) {
    let (task, _run) = create_running_task(&pool, "q").await;
    // Lease is far in the future (default from create_running_task)

    valka_scheduler::reaper::reap_expired_leases(&pool)
        .await
        .unwrap();

    let unchanged = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(
        unchanged.status, "RUNNING",
        "Valid lease should not be reaped"
    );
}

// ─── Dead Letter Processing ─────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_process_dead_letters(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    // Set to FAILED with attempt_count >= max_retries
    tasks::fail_task(&pool, &task.id, "fatal error")
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET attempt_count = max_retries WHERE id = $1")
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();

    let count = valka_scheduler::dlq::process_dead_letters(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Task should be DEAD_LETTER
    let updated = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(updated.status, "DEAD_LETTER");

    // DLQ entry should exist
    let dls = valka_db::queries::dead_letter::list_dead_letters(&pool, None, 50, 0)
        .await
        .unwrap();
    assert_eq!(dls.len(), 1);
    assert_eq!(dls[0].task_id, task.id);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_process_dead_letters_under_max(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    // FAILED but attempt_count=0 < max_retries=3 → should NOT be moved
    tasks::fail_task(&pool, &task.id, "error").await.unwrap();

    let count = valka_scheduler::dlq::process_dead_letters(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    let unchanged = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(unchanged.status, "FAILED");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_process_dead_letters_none(pool: PgPool) {
    // No FAILED tasks
    create_test_task(&pool, "q", "t").await;

    let count = valka_scheduler::dlq::process_dead_letters(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// ─── Additional Reaper Tests ────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reap_expired_lease_inserts_dlq_with_error(pool: PgPool) {
    let (task, run) = create_running_task(&pool, "q").await;

    // Exhaust retries AND set a specific error message on the run
    sqlx::query("UPDATE tasks SET attempt_count = max_retries WHERE id = $1")
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE task_runs SET error_message = 'OOM killed' WHERE id = $1")
        .bind(&run.id)
        .execute(&pool)
        .await
        .unwrap();

    // Expire the lease
    sqlx::query(
        "UPDATE task_runs SET lease_expires_at = NOW() - INTERVAL '1 minute' WHERE id = $1",
    )
    .bind(&run.id)
    .execute(&pool)
    .await
    .unwrap();

    valka_scheduler::reaper::reap_expired_leases(&pool)
        .await
        .unwrap();

    let updated = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(updated.status, "DEAD_LETTER");

    // DLQ entry should exist with the error
    let dls = dead_letter::list_dead_letters(&pool, None, 50, 0)
        .await
        .unwrap();
    assert_eq!(dls.len(), 1);
    assert_eq!(dls[0].task_id, task.id);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reap_multiple_expired_leases_batch(pool: PgPool) {
    // Create 5 expired tasks
    for _ in 0..5 {
        let (task, _run) = create_running_task(&pool, "q").await;
        sqlx::query(
            "UPDATE task_runs SET lease_expires_at = NOW() - INTERVAL '1 minute' WHERE task_id = $1",
        )
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();
    }

    let count = valka_scheduler::reaper::reap_expired_leases(&pool)
        .await
        .unwrap();
    assert_eq!(count, 5);

    // All should be RETRY (attempt_count=0 < max_retries=3)
    let retries = tasks::list_tasks(&pool, None, Some("RETRY"), 50, 0)
        .await
        .unwrap();
    assert_eq!(retries.len(), 5);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reap_expired_lease_run_marked_failed(pool: PgPool) {
    let (_task, run) = create_running_task(&pool, "q").await;

    sqlx::query(
        "UPDATE task_runs SET lease_expires_at = NOW() - INTERVAL '1 minute' WHERE id = $1",
    )
    .bind(&run.id)
    .execute(&pool)
    .await
    .unwrap();

    valka_scheduler::reaper::reap_expired_leases(&pool)
        .await
        .unwrap();

    let run_after = task_runs::get_task_run(&pool, &run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run_after.status, "FAILED");
    assert_eq!(run_after.error_message.as_deref(), Some("Lease expired"));
    assert!(run_after.completed_at.is_some());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reap_boundary_one_under_max_retries(pool: PgPool) {
    let (task, run) = create_running_task(&pool, "q").await;

    // Set attempt_count = max_retries - 1 (should RETRY, not DLQ)
    sqlx::query("UPDATE tasks SET attempt_count = max_retries - 1 WHERE id = $1")
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "UPDATE task_runs SET lease_expires_at = NOW() - INTERVAL '1 minute' WHERE id = $1",
    )
    .bind(&run.id)
    .execute(&pool)
    .await
    .unwrap();

    valka_scheduler::reaper::reap_expired_leases(&pool)
        .await
        .unwrap();

    let updated = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert_eq!(updated.status, "RETRY", "One under max should still retry");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reap_does_not_touch_completed_runs(pool: PgPool) {
    // Create a COMPLETED task with a run
    let task_ok = create_test_task(&pool, "q", "ok-task").await;
    tasks::complete_task(&pool, &task_ok.id, None)
        .await
        .unwrap();

    // Create an expired RUNNING task
    let (task_exp, _run) = create_running_task(&pool, "q").await;
    sqlx::query(
        "UPDATE task_runs SET lease_expires_at = NOW() - INTERVAL '1 minute' WHERE task_id = $1",
    )
    .bind(&task_exp.id)
    .execute(&pool)
    .await
    .unwrap();

    let count = valka_scheduler::reaper::reap_expired_leases(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "Only the expired task should be reaped");

    let ok_after = tasks::get_task(&pool, &task_ok.id).await.unwrap().unwrap();
    assert_eq!(ok_after.status, "COMPLETED", "Completed task untouched");
}

// ─── Additional Retry Tests ─────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_retry_delay_high_attempt_count(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    // Set attempt_count to 50
    for _ in 0..50 {
        tasks::increment_attempt_count(&pool, &task.id)
            .await
            .unwrap();
    }
    tasks::update_task_status(&pool, &task.id, "RETRY")
        .await
        .unwrap();

    // Should not overflow/panic
    let count = valka_scheduler::retry::process_retries(&pool, 1, 3600)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let updated = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert!(updated.scheduled_at.is_some());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_process_retries_batch(pool: PgPool) {
    // Create 5 RETRY tasks with different attempt counts
    for i in 0..5 {
        let task = create_test_task(&pool, "q", &format!("t{i}")).await;
        for _ in 0..i {
            tasks::increment_attempt_count(&pool, &task.id)
                .await
                .unwrap();
        }
        tasks::update_task_status(&pool, &task.id, "RETRY")
            .await
            .unwrap();
    }

    let count = valka_scheduler::retry::process_retries(&pool, 1, 3600)
        .await
        .unwrap();
    assert_eq!(count, 5);

    // All should have scheduled_at set
    let retries = tasks::list_tasks(&pool, None, Some("RETRY"), 50, 0)
        .await
        .unwrap();
    for t in &retries {
        assert!(t.scheduled_at.is_some());
    }
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_retry_then_promote_then_dequeue_cycle(pool: PgPool) {
    let task = create_test_task(&pool, "q", "cycle-task").await;
    let partition = task.partition_id;
    tasks::update_task_status(&pool, &task.id, "RETRY")
        .await
        .unwrap();

    // Process retries → sets scheduled_at
    valka_scheduler::retry::process_retries(&pool, 1, 3600)
        .await
        .unwrap();
    let retrying = tasks::get_task(&pool, &task.id).await.unwrap().unwrap();
    assert!(retrying.scheduled_at.is_some());

    // Time-travel: move scheduled_at to the past
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

    // Dequeue → DISPATCHING
    let dequeued = tasks::dequeue_tasks(&pool, "q", partition, 10)
        .await
        .unwrap();
    assert_eq!(dequeued.len(), 1);
    assert_eq!(dequeued[0].id, task.id);
    assert_eq!(dequeued[0].status, "DISPATCHING");
}

// ─── Additional DLQ Tests ───────────────────────────────────────────

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dlq_preserves_task_input(pool: PgPool) {
    let mut params = default_task_params("q", "t");
    params.input = Some(serde_json::json!({"order_id": 42, "items": ["a", "b"]}));
    let task = create_test_task_full(&pool, params).await;

    tasks::fail_task(&pool, &task.id, "error").await.unwrap();
    sqlx::query("UPDATE tasks SET attempt_count = max_retries WHERE id = $1")
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();

    valka_scheduler::dlq::process_dead_letters(&pool)
        .await
        .unwrap();

    let dls = dead_letter::list_dead_letters(&pool, None, 50, 0)
        .await
        .unwrap();
    assert_eq!(dls.len(), 1);
    let dlq_input = dls[0].input.as_ref().unwrap();
    assert_eq!(dlq_input["order_id"], 42);
    assert_eq!(dlq_input["items"], serde_json::json!(["a", "b"]));
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dlq_no_runs_still_creates_entry(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    // FAILED task with no runs
    tasks::fail_task(&pool, &task.id, "fatal").await.unwrap();
    sqlx::query("UPDATE tasks SET attempt_count = max_retries WHERE id = $1")
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();

    let count = valka_scheduler::dlq::process_dead_letters(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let dls = dead_letter::list_dead_letters(&pool, None, 50, 0)
        .await
        .unwrap();
    assert_eq!(dls.len(), 1);
    // error_message comes from runs — with no runs, it should be None
    assert!(dls[0].error_message.is_none());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_dlq_error_from_latest_run(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    // Create 2 failed runs with different errors
    tasks::update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();
    tasks::increment_attempt_count(&pool, &task.id)
        .await
        .unwrap();
    let run1 = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;
    task_runs::fail_task_run(&pool, &run1.id, "first error")
        .await
        .unwrap();

    tasks::update_task_status(&pool, &task.id, "RUNNING")
        .await
        .unwrap();
    tasks::increment_attempt_count(&pool, &task.id)
        .await
        .unwrap();
    let run2 = create_test_run(&pool, &task.id, 2, Utc::now() + Duration::seconds(300)).await;
    task_runs::fail_task_run(&pool, &run2.id, "second error")
        .await
        .unwrap();

    // Fail the task and exhaust retries
    tasks::fail_task(&pool, &task.id, "final").await.unwrap();
    sqlx::query("UPDATE tasks SET attempt_count = max_retries WHERE id = $1")
        .bind(&task.id)
        .execute(&pool)
        .await
        .unwrap();

    valka_scheduler::dlq::process_dead_letters(&pool)
        .await
        .unwrap();

    let dls = dead_letter::list_dead_letters(&pool, None, 50, 0)
        .await
        .unwrap();
    assert_eq!(dls.len(), 1);
    // get_runs_for_task orders by attempt_number DESC, so latest run's error is first
    assert_eq!(dls[0].error_message.as_deref(), Some("second error"));
}
