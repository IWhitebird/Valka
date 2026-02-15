use sqlx::PgPool;
use valka_db::queries::signals::*;

use super::helpers::*;

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_create_signal(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    let signal = create_signal(&pool, "sig-1", &task.id, "approve", None)
        .await
        .unwrap();

    assert_eq!(signal.id, "sig-1");
    assert_eq!(signal.task_id, task.id);
    assert_eq!(signal.signal_name, "approve");
    assert!(signal.payload.is_none());
    assert_eq!(signal.status, "PENDING");
    assert!(signal.delivered_at.is_none());
    assert!(signal.acknowledged_at.is_none());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_create_signal_with_json_payload(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    let payload = serde_json::json!({"key": "value", "count": 42});
    let signal = create_signal(&pool, "sig-json", &task.id, "data", Some(payload.clone()))
        .await
        .unwrap();

    assert_eq!(signal.payload.unwrap(), payload);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_create_signal_null_payload(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    let signal = create_signal(&pool, "sig-null", &task.id, "ping", None)
        .await
        .unwrap();

    assert!(signal.payload.is_none());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_pending_signals_ordered(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    // Insert 3 signals — created_at is set by DB, so order = insertion order
    create_signal(&pool, "s1", &task.id, "first", None).await.unwrap();
    create_signal(&pool, "s2", &task.id, "second", None).await.unwrap();
    create_signal(&pool, "s3", &task.id, "third", None).await.unwrap();

    let signals = get_pending_signals(&pool, &task.id).await.unwrap();
    assert_eq!(signals.len(), 3);
    assert_eq!(signals[0].signal_name, "first");
    assert_eq!(signals[1].signal_name, "second");
    assert_eq!(signals[2].signal_name, "third");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_pending_signals_excludes_delivered(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    create_signal(&pool, "s-pending", &task.id, "a", None).await.unwrap();
    create_signal(&pool, "s-delivered", &task.id, "b", None).await.unwrap();

    mark_delivered(&pool, "s-delivered").await.unwrap();

    let signals = get_pending_signals(&pool, &task.id).await.unwrap();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].id, "s-pending");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_pending_signals_empty(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    let signals = get_pending_signals(&pool, &task.id).await.unwrap();
    assert!(signals.is_empty());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_mark_delivered(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    create_signal(&pool, "sig-d", &task.id, "test", None).await.unwrap();

    let affected = mark_delivered(&pool, "sig-d").await.unwrap();
    assert!(affected, "Should mark PENDING signal as DELIVERED");

    // Verify status changed
    let signals = list_signals(&pool, &task.id, Some("DELIVERED")).await.unwrap();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].status, "DELIVERED");
    assert!(signals[0].delivered_at.is_some());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_mark_delivered_idempotent(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    create_signal(&pool, "sig-idem", &task.id, "test", None).await.unwrap();

    mark_delivered(&pool, "sig-idem").await.unwrap();

    // Second mark should return false (already DELIVERED)
    let affected = mark_delivered(&pool, "sig-idem").await.unwrap();
    assert!(!affected, "Should return false for already-delivered signal");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_mark_acknowledged(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    create_signal(&pool, "sig-ack", &task.id, "test", None).await.unwrap();

    mark_delivered(&pool, "sig-ack").await.unwrap();
    let affected = mark_acknowledged(&pool, "sig-ack").await.unwrap();
    assert!(affected, "Should mark DELIVERED signal as ACKNOWLEDGED");

    let signals = list_signals(&pool, &task.id, Some("ACKNOWLEDGED")).await.unwrap();
    assert_eq!(signals.len(), 1);
    assert!(signals[0].acknowledged_at.is_some());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_mark_acknowledged_requires_delivered(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    create_signal(&pool, "sig-pending-ack", &task.id, "test", None)
        .await
        .unwrap();

    // Try to acknowledge a PENDING signal
    let affected = mark_acknowledged(&pool, "sig-pending-ack").await.unwrap();
    assert!(
        !affected,
        "Should return false when trying to acknowledge a PENDING signal"
    );
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reset_delivered_signals(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    create_signal(&pool, "s-pending", &task.id, "a", None).await.unwrap();
    create_signal(&pool, "s-delivered", &task.id, "b", None).await.unwrap();
    create_signal(&pool, "s-acked", &task.id, "c", None).await.unwrap();

    mark_delivered(&pool, "s-delivered").await.unwrap();
    mark_delivered(&pool, "s-acked").await.unwrap();
    mark_acknowledged(&pool, "s-acked").await.unwrap();

    let reset_count = reset_delivered_signals(&pool, &task.id).await.unwrap();
    assert_eq!(reset_count, 1, "Only the DELIVERED signal should be reset");

    // Verify the reset signal is now PENDING
    let pending = get_pending_signals(&pool, &task.id).await.unwrap();
    assert_eq!(pending.len(), 2); // original pending + reset one
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_reset_delivered_signals_none_to_reset(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    create_signal(&pool, "s1", &task.id, "a", None).await.unwrap();
    create_signal(&pool, "s2", &task.id, "b", None).await.unwrap();

    let reset_count = reset_delivered_signals(&pool, &task.id).await.unwrap();
    assert_eq!(reset_count, 0, "No DELIVERED signals to reset");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_signals_all(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    create_signal(&pool, "s1", &task.id, "a", None).await.unwrap();
    create_signal(&pool, "s2", &task.id, "b", None).await.unwrap();
    create_signal(&pool, "s3", &task.id, "c", None).await.unwrap();
    mark_delivered(&pool, "s2").await.unwrap();
    mark_delivered(&pool, "s3").await.unwrap();
    mark_acknowledged(&pool, "s3").await.unwrap();

    let all = list_signals(&pool, &task.id, None).await.unwrap();
    assert_eq!(all.len(), 3);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_signals_filter_status(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    create_signal(&pool, "s1", &task.id, "a", None).await.unwrap();
    create_signal(&pool, "s2", &task.id, "b", None).await.unwrap();
    create_signal(&pool, "s3", &task.id, "c", None).await.unwrap();
    mark_delivered(&pool, "s2").await.unwrap();

    let pending = list_signals(&pool, &task.id, Some("PENDING")).await.unwrap();
    assert_eq!(pending.len(), 2);

    let delivered = list_signals(&pool, &task.id, Some("DELIVERED")).await.unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].id, "s2");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_signals_empty(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    let signals = list_signals(&pool, &task.id, None).await.unwrap();
    assert!(signals.is_empty());
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_signal_cascade_on_task_delete(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;

    create_signal(&pool, "s1", &task.id, "a", None).await.unwrap();
    create_signal(&pool, "s2", &task.id, "b", None).await.unwrap();

    // Delete the task — signals should cascade delete
    valka_db::queries::tasks::delete_task(&pool, &task.id)
        .await
        .unwrap();

    let signals = list_signals(&pool, &task.id, None).await.unwrap();
    assert!(signals.is_empty(), "Signals should be cascade-deleted with task");
}
