use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SignalRow {
    pub id: String,
    pub task_id: String,
    pub signal_name: String,
    pub payload: Option<serde_json::Value>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub acknowledged_at: Option<DateTime<Utc>>,
}

pub async fn create_signal(
    pool: &PgPool,
    id: &str,
    task_id: &str,
    signal_name: &str,
    payload: Option<serde_json::Value>,
) -> Result<SignalRow, sqlx::Error> {
    sqlx::query_as::<_, SignalRow>(
        r#"
        INSERT INTO task_signals (id, task_id, signal_name, payload)
        VALUES ($1, $2, $3, $4)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(task_id)
    .bind(signal_name)
    .bind(&payload)
    .fetch_one(pool)
    .await
}

pub async fn get_pending_signals(
    pool: &PgPool,
    task_id: &str,
) -> Result<Vec<SignalRow>, sqlx::Error> {
    sqlx::query_as::<_, SignalRow>(
        "SELECT * FROM task_signals WHERE task_id = $1 AND status = 'PENDING' ORDER BY created_at ASC",
    )
    .bind(task_id)
    .fetch_all(pool)
    .await
}

pub async fn mark_delivered(pool: &PgPool, signal_id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE task_signals SET status = 'DELIVERED', delivered_at = NOW() WHERE id = $1 AND status = 'PENDING'",
    )
    .bind(signal_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn mark_acknowledged(pool: &PgPool, signal_id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE task_signals SET status = 'ACKNOWLEDGED', acknowledged_at = NOW() WHERE id = $1 AND status = 'DELIVERED'",
    )
    .bind(signal_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn reset_delivered_signals(pool: &PgPool, task_id: &str) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE task_signals SET status = 'PENDING', delivered_at = NULL WHERE task_id = $1 AND status = 'DELIVERED'",
    )
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn list_signals(
    pool: &PgPool,
    task_id: &str,
    status_filter: Option<&str>,
) -> Result<Vec<SignalRow>, sqlx::Error> {
    if let Some(status) = status_filter {
        sqlx::query_as::<_, SignalRow>(
            "SELECT * FROM task_signals WHERE task_id = $1 AND status = $2 ORDER BY created_at ASC",
        )
        .bind(task_id)
        .bind(status)
        .fetch_all(pool)
        .await
    } else {
        sqlx::query_as::<_, SignalRow>(
            "SELECT * FROM task_signals WHERE task_id = $1 ORDER BY created_at ASC",
        )
        .bind(task_id)
        .fetch_all(pool)
        .await
    }
}
