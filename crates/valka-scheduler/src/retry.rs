use chrono::{Duration, Utc};
use sqlx::PgPool;
use tracing::{error, info};
use valka_db::queries::tasks;

/// Compute exponential backoff delay for a retry attempt
pub fn compute_retry_delay(
    attempt_count: i32,
    base_delay_secs: u64,
    max_delay_secs: u64,
) -> Duration {
    let delay_secs = base_delay_secs.saturating_mul(2u64.saturating_pow(attempt_count as u32));
    let capped = delay_secs.min(max_delay_secs);
    Duration::seconds(capped as i64)
}

/// Process tasks in RETRY status: compute next attempt time and set scheduled_at
pub async fn process_retries(
    pool: &PgPool,
    base_delay_secs: u64,
    max_delay_secs: u64,
) -> Result<usize, sqlx::Error> {
    // Find RETRY tasks that don't have a scheduled_at yet
    let rows = sqlx::query_as::<_, tasks::TaskRow>(
        r#"
        SELECT * FROM tasks
        WHERE status = 'RETRY' AND scheduled_at IS NULL
        LIMIT 100
        "#,
    )
    .fetch_all(pool)
    .await?;

    let count = rows.len();

    for task in rows {
        let delay = compute_retry_delay(task.attempt_count, base_delay_secs, max_delay_secs);
        let scheduled_at = Utc::now() + delay;

        if let Err(e) = tasks::schedule_retry(pool, &task.id, scheduled_at).await {
            error!(task_id = %task.id, error = %e, "Failed to schedule retry");
        } else {
            info!(
                task_id = %task.id,
                attempt = task.attempt_count,
                next_at = %scheduled_at,
                "Scheduled retry"
            );
        }
    }

    Ok(count)
}
