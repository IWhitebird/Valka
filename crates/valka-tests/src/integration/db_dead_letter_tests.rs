use sqlx::PgPool;
use valka_db::queries::dead_letter::*;

use super::helpers::*;

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_insert_dead_letter(pool: PgPool) {
    let task = create_test_task(&pool, "q", "t").await;
    let dlq_id = uuid::Uuid::now_v7().to_string();

    let dl = insert_dead_letter(
        &pool,
        &dlq_id,
        &task.id,
        "q",
        "t",
        task.input.as_ref(),
        Some("max retries"),
        3,
        &serde_json::json!({"source": "test"}),
    )
    .await
    .unwrap();

    assert_eq!(dl.id, dlq_id);
    assert_eq!(dl.task_id, task.id);
    assert_eq!(dl.queue_name, "q");
    assert_eq!(dl.task_name, "t");
    assert_eq!(dl.error_message.as_deref(), Some("max retries"));
    assert_eq!(dl.attempt_count, 3);
    assert_eq!(dl.metadata["source"], "test");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_insert_dead_letter_nonexistent_task(pool: PgPool) {
    let result = insert_dead_letter(
        &pool,
        &uuid::Uuid::now_v7().to_string(),
        "nonexistent-task",
        "q",
        "t",
        None,
        None,
        0,
        &serde_json::json!({}),
    )
    .await;
    assert!(result.is_err(), "FK violation should fail");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_dead_letters_all(pool: PgPool) {
    for i in 0..3 {
        let task = create_test_task(&pool, "q", &format!("t{i}")).await;
        insert_dead_letter(
            &pool,
            &uuid::Uuid::now_v7().to_string(),
            &task.id,
            "q",
            &format!("t{i}"),
            None,
            Some("error"),
            i + 1,
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }

    let dls = list_dead_letters(&pool, None, 50, 0).await.unwrap();
    assert_eq!(dls.len(), 3);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_dead_letters_filter_by_queue(pool: PgPool) {
    let task_a = create_test_task(&pool, "queue-a", "t").await;
    let task_b = create_test_task(&pool, "queue-b", "t").await;

    insert_dead_letter(
        &pool,
        &uuid::Uuid::now_v7().to_string(),
        &task_a.id,
        "queue-a",
        "t",
        None,
        None,
        1,
        &serde_json::json!({}),
    )
    .await
    .unwrap();

    insert_dead_letter(
        &pool,
        &uuid::Uuid::now_v7().to_string(),
        &task_b.id,
        "queue-b",
        "t",
        None,
        None,
        1,
        &serde_json::json!({}),
    )
    .await
    .unwrap();

    let a_dls = list_dead_letters(&pool, Some("queue-a"), 50, 0)
        .await
        .unwrap();
    assert_eq!(a_dls.len(), 1);
    assert_eq!(a_dls[0].queue_name, "queue-a");
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_dead_letters_pagination(pool: PgPool) {
    for i in 0..5 {
        let task = create_test_task(&pool, "q", &format!("t{i}")).await;
        insert_dead_letter(
            &pool,
            &uuid::Uuid::now_v7().to_string(),
            &task.id,
            "q",
            &format!("t{i}"),
            None,
            None,
            1,
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }

    let page = list_dead_letters(&pool, None, 2, 2).await.unwrap();
    assert_eq!(page.len(), 2);

    let all = list_dead_letters(&pool, None, 50, 0).await.unwrap();
    assert_eq!(all.len(), 5);
}

#[sqlx::test(migrations = "../../crates/valka-db/migrations")]
async fn test_list_dead_letters_empty(pool: PgPool) {
    let dls = list_dead_letters(&pool, None, 50, 0).await.unwrap();
    assert!(dls.is_empty());
}
