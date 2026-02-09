use chrono::{Duration, Utc};
use sqlx::PgPool;
use valka_db::queries::task_logs::*;

use super::helpers::*;

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_batch_insert_logs(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;

    let entries: Vec<InsertLogEntry> = (0..5)
        .map(|i| InsertLogEntry {
            task_run_id: run.id.clone(),
            timestamp_ms: 1000 + i,
            level: "INFO".to_string(),
            message: format!("Log message {i}"),
            metadata: Some(serde_json::json!({"index": i})),
        })
        .collect();

    let count = batch_insert_logs(&pool, &entries).await.unwrap();
    assert_eq!(count, 5);

    // Verify all stored
    let logs = get_logs_for_run(&pool, &run.id, 100, None).await.unwrap();
    assert_eq!(logs.len(), 5);
    assert_eq!(logs[0].message, "Log message 0");
    assert_eq!(logs[4].message, "Log message 4");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_batch_insert_logs_empty(pool: PgPool) {
    let count = batch_insert_logs(&pool, &[]).await.unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_logs_for_run_ordered(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;

    let entries = vec![
        InsertLogEntry {
            task_run_id: run.id.clone(),
            timestamp_ms: 3000,
            level: "ERROR".to_string(),
            message: "third".to_string(),
            metadata: None,
        },
        InsertLogEntry {
            task_run_id: run.id.clone(),
            timestamp_ms: 1000,
            level: "INFO".to_string(),
            message: "first".to_string(),
            metadata: None,
        },
        InsertLogEntry {
            task_run_id: run.id.clone(),
            timestamp_ms: 2000,
            level: "WARN".to_string(),
            message: "second".to_string(),
            metadata: None,
        },
    ];
    batch_insert_logs(&pool, &entries).await.unwrap();

    let logs = get_logs_for_run(&pool, &run.id, 100, None).await.unwrap();
    assert_eq!(logs.len(), 3);
    // Ordered by timestamp_ms ASC
    assert_eq!(logs[0].message, "first");
    assert_eq!(logs[1].message, "second");
    assert_eq!(logs[2].message, "third");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_logs_pagination_after_id(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;

    let entries: Vec<InsertLogEntry> = (0..5)
        .map(|i| InsertLogEntry {
            task_run_id: run.id.clone(),
            timestamp_ms: 1000 + i,
            level: "INFO".to_string(),
            message: format!("msg-{i}"),
            metadata: None,
        })
        .collect();
    batch_insert_logs(&pool, &entries).await.unwrap();

    // Get first page
    let page1 = get_logs_for_run(&pool, &run.id, 2, None).await.unwrap();
    assert_eq!(page1.len(), 2);

    // Get second page using after_id cursor
    let after_id = page1.last().unwrap().id;
    let page2 = get_logs_for_run(&pool, &run.id, 2, Some(after_id))
        .await
        .unwrap();
    assert_eq!(page2.len(), 2);
    // Should not overlap
    assert!(page2[0].id > after_id);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_logs_limit(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let run = create_test_run(&pool, &task.id, 1, Utc::now() + Duration::seconds(300)).await;

    let entries: Vec<InsertLogEntry> = (0..10)
        .map(|i| InsertLogEntry {
            task_run_id: run.id.clone(),
            timestamp_ms: 1000 + i,
            level: "INFO".to_string(),
            message: format!("msg-{i}"),
            metadata: None,
        })
        .collect();
    batch_insert_logs(&pool, &entries).await.unwrap();

    let logs = get_logs_for_run(&pool, &run.id, 3, None).await.unwrap();
    assert_eq!(logs.len(), 3);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_get_logs_no_logs(pool: PgPool) {
    let logs = get_logs_for_run(&pool, "nonexistent-run", 100, None)
        .await
        .unwrap();
    assert!(logs.is_empty());
}
