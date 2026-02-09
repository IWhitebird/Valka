use chrono::{Duration, Utc};
use sqlx::PgPool;
use valka_db::queries::task_runs::*;

use super::helpers::*;

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_create_task_run(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let lease = Utc::now() + Duration::seconds(300);

    let run = create_test_run(&pool, &task.id, 1, lease).await;

    assert_eq!(run.task_id, task.id);
    assert_eq!(run.attempt_number, 1);
    assert_eq!(run.status, "RUNNING");
    assert!(run.completed_at.is_none());
    assert!(run.output.is_none());
    assert!(run.error_message.is_none());
    assert!(!run.id.is_empty());
    assert!(!run.worker_id.is_empty());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_create_task_run_duplicate_attempt(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let lease = Utc::now() + Duration::seconds(300);

    create_test_run(&pool, &task.id, 1, lease).await;

    // Same task_id + attempt_number should fail (UNIQUE constraint)
    let result = create_task_run(
        &pool,
        CreateTaskRunParams {
            id: uuid::Uuid::now_v7().to_string(),
            task_id: task.id.clone(),
            attempt_number: 1,
            worker_id: uuid::Uuid::now_v7().to_string(),
            assigned_node_id: uuid::Uuid::now_v7().to_string(),
            lease_expires_at: lease,
        },
    )
    .await;
    assert!(
        result.is_err(),
        "Duplicate (task_id, attempt_number) should fail"
    );
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_create_task_run_nonexistent_task(pool: PgPool) {
    let result = create_task_run(
        &pool,
        CreateTaskRunParams {
            id: uuid::Uuid::now_v7().to_string(),
            task_id: "nonexistent-task".to_string(),
            attempt_number: 1,
            worker_id: uuid::Uuid::now_v7().to_string(),
            assigned_node_id: uuid::Uuid::now_v7().to_string(),
            lease_expires_at: Utc::now() + Duration::seconds(300),
        },
    )
    .await;
    assert!(result.is_err(), "FK violation should fail");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_complete_task_run(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;
    let output = serde_json::json!({"result": "ok"});

    let completed = complete_task_run(&pool, &run.id, Some(output.clone()))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(completed.status, "COMPLETED");
    assert_eq!(completed.output.unwrap(), output);
    assert!(completed.completed_at.is_some());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_complete_task_run_already_completed(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;

    complete_task_run(&pool, &run.id, None).await.unwrap();
    // Second complete should return None (WHERE status = 'RUNNING' no longer matches)
    let second = complete_task_run(&pool, &run.id, None).await.unwrap();
    assert!(second.is_none(), "Already completed run should return None");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_fail_task_run(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;

    let failed = fail_task_run(&pool, &run.id, "timeout")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(failed.status, "FAILED");
    assert_eq!(failed.error_message.as_deref(), Some("timeout"));
    assert!(failed.completed_at.is_some());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_fail_task_run_already_failed(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;

    fail_task_run(&pool, &run.id, "error").await.unwrap();
    let second = fail_task_run(&pool, &run.id, "error2").await.unwrap();
    assert!(second.is_none(), "Already failed run should return None");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_update_heartbeat(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(60)).await;

    let new_lease = Utc::now() + Duration::seconds(120);
    let updated = update_heartbeat(&pool, &run.id, new_lease).await.unwrap();
    assert!(updated, "Should update running run");

    // Verify lease was extended
    let fetched = get_task_run(&pool, &run.id).await.unwrap().unwrap();
    assert!(fetched.lease_expires_at > run.lease_expires_at);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_update_heartbeat_not_running(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;

    // Complete the run first
    complete_task_run(&pool, &run.id, None).await.unwrap();

    // Now heartbeat should not update
    let new_lease = Utc::now() + Duration::seconds(600);
    let updated = update_heartbeat(&pool, &run.id, new_lease).await.unwrap();
    assert!(!updated, "Completed run should not be updated");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_update_heartbeat_by_task(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run1 = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(60)).await;
    // Complete run1
    complete_task_run(&pool, &run1.id, None).await.unwrap();

    // Create a second RUNNING run
    let run2 = create_test_run(&pool, &task.id, 2, Utc::now() + Duration::seconds(60)).await;

    let new_lease = Utc::now() + Duration::seconds(300);
    let updated = update_heartbeat_by_task(&pool, &task.id, new_lease)
        .await
        .unwrap();
    assert!(updated, "Should update the RUNNING run");

    // Verify only run2 was updated
    let fetched2 = get_task_run(&pool, &run2.id).await.unwrap().unwrap();
    assert!(fetched2.lease_expires_at > run2.lease_expires_at);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_find_expired_leases(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    // Create a run with lease in the past
    let _expired = create_test_run(&pool, &task.id, 1, Utc::now() - Duration::seconds(10)).await;

    let expired = find_expired_leases(&pool).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].task_id, task.id);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_find_expired_leases_none(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    // Lease far in the future
    create_test_run(&pool, &task.id, 1, Utc::now() + Duration::hours(1)).await;

    let expired = find_expired_leases(&pool).await.unwrap();
    assert!(expired.is_empty());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_runs_for_task(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let lease = Utc::now() + Duration::seconds(300);
    create_test_run(&pool, &task.id, 1, lease).await;
    create_test_run(&pool, &task.id, 2, lease).await;
    create_test_run(&pool, &task.id, 3, lease).await;

    let runs = get_runs_for_task(&pool, &task.id).await.unwrap();
    assert_eq!(runs.len(), 3);
    // Ordered by attempt_number DESC
    assert_eq!(runs[0].attempt_number, 3);
    assert_eq!(runs[1].attempt_number, 2);
    assert_eq!(runs[2].attempt_number, 1);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_runs_for_task_empty(pool: PgPool) {
    let runs = get_runs_for_task(&pool, "nonexistent").await.unwrap();
    assert!(runs.is_empty());
}
