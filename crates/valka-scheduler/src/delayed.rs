use sqlx::PgPool;
use tracing::info;
use valka_db::queries::tasks;

/// Promote delayed/retry tasks whose scheduled_at has passed back to PENDING
pub async fn promote_delayed_tasks(pool: &PgPool) -> Result<usize, sqlx::Error> {
    let promoted = tasks::promote_delayed_tasks(pool).await?;
    let count = promoted.len();

    if count > 0 {
        info!(count, "Promoted delayed tasks to PENDING");
    }

    Ok(count)
}
