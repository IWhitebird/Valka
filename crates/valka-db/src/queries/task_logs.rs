use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TaskLogRow {
    pub id: i64,
    pub task_run_id: String,
    pub timestamp_ms: i64,
    pub level: String,
    pub message: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct InsertLogEntry {
    pub task_run_id: String,
    pub timestamp_ms: i64,
    pub level: String,
    pub message: String,
    pub metadata: Option<serde_json::Value>,
}

/// Batch insert log entries
pub async fn batch_insert_logs(
    pool: &PgPool,
    entries: &[InsertLogEntry],
) -> Result<u64, sqlx::Error> {
    if entries.is_empty() {
        return Ok(0);
    }

    // Build a bulk INSERT using UNNEST for efficiency
    let task_run_ids: Vec<&str> = entries.iter().map(|e| e.task_run_id.as_str()).collect();
    let timestamps: Vec<i64> = entries.iter().map(|e| e.timestamp_ms).collect();
    let levels: Vec<&str> = entries.iter().map(|e| e.level.as_str()).collect();
    let messages: Vec<&str> = entries.iter().map(|e| e.message.as_str()).collect();
    let metadata: Vec<Option<serde_json::Value>> =
        entries.iter().map(|e| e.metadata.clone()).collect();

    let result = sqlx::query(
        r#"
        INSERT INTO task_logs (task_run_id, timestamp_ms, level, message, metadata)
        SELECT * FROM UNNEST($1::text[], $2::bigint[], $3::text[], $4::text[], $5::jsonb[])
        "#,
    )
    .bind(&task_run_ids)
    .bind(&timestamps)
    .bind(&levels)
    .bind(&messages)
    .bind(&metadata)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Get logs for a task run
pub async fn get_logs_for_run(
    pool: &PgPool,
    task_run_id: &str,
    limit: i64,
    after_id: Option<i64>,
) -> Result<Vec<TaskLogRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TaskLogRow>(
        r#"
        SELECT * FROM task_logs
        WHERE task_run_id = $1 AND ($3::bigint IS NULL OR id > $3)
        ORDER BY timestamp_ms ASC
        LIMIT $2
        "#,
    )
    .bind(task_run_id)
    .bind(limit)
    .bind(after_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
