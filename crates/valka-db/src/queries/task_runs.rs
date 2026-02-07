use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TaskRunRow {
    pub id: String,
    pub task_id: String,
    pub attempt_number: i32,
    pub worker_id: String,
    pub assigned_node_id: String,
    pub status: String,
    pub output: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub lease_expires_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_heartbeat: DateTime<Utc>,
}

pub struct CreateTaskRunParams {
    pub id: String,
    pub task_id: String,
    pub attempt_number: i32,
    pub worker_id: String,
    pub assigned_node_id: String,
    pub lease_expires_at: DateTime<Utc>,
}

pub async fn create_task_run(
    pool: &PgPool,
    params: CreateTaskRunParams,
) -> Result<TaskRunRow, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRunRow>(
        r#"
        INSERT INTO task_runs (id, task_id, attempt_number, worker_id, assigned_node_id, lease_expires_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING *
        "#,
    )
    .bind(&params.id)
    .bind(&params.task_id)
    .bind(params.attempt_number)
    .bind(&params.worker_id)
    .bind(&params.assigned_node_id)
    .bind(params.lease_expires_at)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn complete_task_run(
    pool: &PgPool,
    run_id: &str,
    output: Option<serde_json::Value>,
) -> Result<Option<TaskRunRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRunRow>(
        r#"
        UPDATE task_runs SET status = 'COMPLETED', output = $2, completed_at = NOW()
        WHERE id = $1 AND status = 'RUNNING'
        RETURNING *
        "#,
    )
    .bind(run_id)
    .bind(output)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn fail_task_run(
    pool: &PgPool,
    run_id: &str,
    error_message: &str,
) -> Result<Option<TaskRunRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRunRow>(
        r#"
        UPDATE task_runs SET status = 'FAILED', error_message = $2, completed_at = NOW()
        WHERE id = $1 AND status = 'RUNNING'
        RETURNING *
        "#,
    )
    .bind(run_id)
    .bind(error_message)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn update_heartbeat(
    pool: &PgPool,
    run_id: &str,
    new_lease_expires_at: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE task_runs SET last_heartbeat = NOW(), lease_expires_at = $2
        WHERE id = $1 AND status = 'RUNNING'
        "#,
    )
    .bind(run_id)
    .bind(new_lease_expires_at)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Find expired leases for the reaper
pub async fn find_expired_leases(pool: &PgPool) -> Result<Vec<TaskRunRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TaskRunRow>(
        r#"
        SELECT * FROM task_runs
        WHERE status = 'RUNNING' AND lease_expires_at < NOW()
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Update heartbeat for all running runs of a given task
pub async fn update_heartbeat_by_task(
    pool: &PgPool,
    task_id: &str,
    new_lease_expires_at: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE task_runs SET last_heartbeat = NOW(), lease_expires_at = $2
        WHERE task_id = $1 AND status = 'RUNNING'
        "#,
    )
    .bind(task_id)
    .bind(new_lease_expires_at)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn get_task_run(pool: &PgPool, run_id: &str) -> Result<Option<TaskRunRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRunRow>("SELECT * FROM task_runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn get_runs_for_task(
    pool: &PgPool,
    task_id: &str,
) -> Result<Vec<TaskRunRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TaskRunRow>(
        "SELECT * FROM task_runs WHERE task_id = $1 ORDER BY attempt_number DESC",
    )
    .bind(task_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
