use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeadLetterRow {
    pub id: String,
    pub task_id: String,
    pub queue_name: String,
    pub task_name: String,
    pub input: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub attempt_count: i32,
    pub metadata: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn insert_dead_letter(
    pool: &PgPool,
    id: &str,
    task_id: &str,
    queue_name: &str,
    task_name: &str,
    input: Option<&serde_json::Value>,
    error_message: Option<&str>,
    attempt_count: i32,
    metadata: &serde_json::Value,
) -> Result<DeadLetterRow, sqlx::Error> {
    let row = sqlx::query_as::<_, DeadLetterRow>(
        r#"
        INSERT INTO dead_letter_queue (id, task_id, queue_name, task_name, input, error_message, attempt_count, metadata)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(task_id)
    .bind(queue_name)
    .bind(task_name)
    .bind(input)
    .bind(error_message)
    .bind(attempt_count)
    .bind(metadata)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn list_dead_letters(
    pool: &PgPool,
    queue_name: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<DeadLetterRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DeadLetterRow>(
        r#"
        SELECT * FROM dead_letter_queue
        WHERE ($1::text IS NULL OR queue_name = $1)
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(queue_name)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
