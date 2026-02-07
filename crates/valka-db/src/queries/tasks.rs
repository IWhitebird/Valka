use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TaskRow {
    pub id: String,
    pub queue_name: String,
    pub task_name: String,
    pub partition_id: i32,
    pub status: String,
    pub input: Option<serde_json::Value>,
    pub priority: i32,
    pub max_retries: i32,
    pub attempt_count: i32,
    pub timeout_seconds: i32,
    pub idempotency_key: Option<String>,
    pub metadata: serde_json::Value,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct CreateTaskParams {
    pub id: String,
    pub queue_name: String,
    pub task_name: String,
    pub partition_id: i32,
    pub input: Option<serde_json::Value>,
    pub priority: i32,
    pub max_retries: i32,
    pub timeout_seconds: i32,
    pub idempotency_key: Option<String>,
    pub metadata: serde_json::Value,
    pub scheduled_at: Option<DateTime<Utc>>,
}

pub async fn create_task(pool: &PgPool, params: CreateTaskParams) -> Result<TaskRow, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRow>(
        r#"
        INSERT INTO tasks (id, queue_name, task_name, partition_id, input, priority, max_retries,
                          timeout_seconds, idempotency_key, metadata, scheduled_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING *
        "#,
    )
    .bind(&params.id)
    .bind(&params.queue_name)
    .bind(&params.task_name)
    .bind(params.partition_id)
    .bind(&params.input)
    .bind(params.priority)
    .bind(params.max_retries)
    .bind(params.timeout_seconds)
    .bind(&params.idempotency_key)
    .bind(&params.metadata)
    .bind(params.scheduled_at)
    .fetch_one(pool)
    .await?;

    Ok(row)
}

pub async fn get_task(pool: &PgPool, task_id: &str) -> Result<Option<TaskRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE id = $1")
        .bind(task_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn list_tasks(
    pool: &PgPool,
    queue_name: Option<&str>,
    status: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TaskRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TaskRow>(
        r#"
        SELECT * FROM tasks
        WHERE ($1::text IS NULL OR queue_name = $1)
          AND ($2::text IS NULL OR status = $2)
        ORDER BY created_at DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(queue_name)
    .bind(status)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn update_task_status(
    pool: &PgPool,
    task_id: &str,
    new_status: &str,
) -> Result<Option<TaskRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRow>(
        r#"
        UPDATE tasks SET status = $2, updated_at = NOW()
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(task_id)
    .bind(new_status)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn increment_attempt_count(
    pool: &PgPool,
    task_id: &str,
) -> Result<Option<TaskRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRow>(
        r#"
        UPDATE tasks SET attempt_count = attempt_count + 1, updated_at = NOW()
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// SKIP LOCKED dequeue: fetch a batch of PENDING tasks for a given queue/partition
pub async fn dequeue_tasks(
    pool: &PgPool,
    queue_name: &str,
    partition_id: i32,
    batch_size: i64,
) -> Result<Vec<TaskRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TaskRow>(
        r#"
        UPDATE tasks SET status = 'DISPATCHING', updated_at = NOW()
        WHERE id IN (
            SELECT id FROM tasks
            WHERE queue_name = $1 AND partition_id = $2 AND status = 'PENDING'
              AND (scheduled_at IS NULL OR scheduled_at <= NOW())
            ORDER BY priority DESC, created_at ASC
            LIMIT $3
            FOR UPDATE SKIP LOCKED
        )
        RETURNING *
        "#,
    )
    .bind(queue_name)
    .bind(partition_id)
    .bind(batch_size)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Cancel a task (only if PENDING or RETRY)
pub async fn cancel_task(pool: &PgPool, task_id: &str) -> Result<Option<TaskRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRow>(
        r#"
        UPDATE tasks SET status = 'CANCELLED', updated_at = NOW()
        WHERE id = $1 AND status IN ('PENDING', 'RETRY')
        RETURNING *
        "#,
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Set task to RETRY with a scheduled_at for next attempt
pub async fn schedule_retry(
    pool: &PgPool,
    task_id: &str,
    scheduled_at: DateTime<Utc>,
) -> Result<Option<TaskRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRow>(
        r#"
        UPDATE tasks SET status = 'RETRY', scheduled_at = $2, updated_at = NOW()
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(task_id)
    .bind(scheduled_at)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Move task to DEAD_LETTER status
pub async fn move_to_dead_letter(
    pool: &PgPool,
    task_id: &str,
) -> Result<Option<TaskRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRow>(
        r#"
        UPDATE tasks SET status = 'DEAD_LETTER', updated_at = NOW()
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Promote RETRY tasks whose scheduled_at has passed back to PENDING
pub async fn promote_delayed_tasks(pool: &PgPool) -> Result<Vec<TaskRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TaskRow>(
        r#"
        UPDATE tasks SET status = 'PENDING', scheduled_at = NULL, updated_at = NOW()
        WHERE status = 'RETRY' AND scheduled_at <= NOW()
        RETURNING *
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Find DISPATCHING tasks with no active runs (crash recovery)
pub async fn recover_orphaned_dispatching(pool: &PgPool) -> Result<Vec<TaskRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TaskRow>(
        r#"
        UPDATE tasks SET status = 'PENDING', updated_at = NOW()
        WHERE status = 'DISPATCHING'
          AND id NOT IN (SELECT task_id FROM task_runs WHERE status = 'RUNNING')
        RETURNING *
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
