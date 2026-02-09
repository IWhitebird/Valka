use chrono::{Duration, Utc};
use sqlx::PgPool;
use valka_db::queries::tasks;

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
