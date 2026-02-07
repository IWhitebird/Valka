use sqlx::PgPool;
use tracing::{error, info};
use uuid::Uuid;
use valka_db::queries::{dead_letter, tasks};

/// Find tasks that have exceeded max_retries and move them to dead letter queue
pub async fn process_dead_letters(pool: &PgPool) -> Result<usize, sqlx::Error> {
    let rows = sqlx::query_as::<_, tasks::TaskRow>(
        r#"
        SELECT * FROM tasks
        WHERE status = 'FAILED' AND attempt_count >= max_retries
        LIMIT 100
        "#,
    )
    .fetch_all(pool)
    .await?;

    let count = rows.len();

    for task in rows {
        let dlq_id = Uuid::now_v7().to_string();

        // Get the last error message from task_runs
        let runs = valka_db::queries::task_runs::get_runs_for_task(pool, &task.id).await?;
        let error_message = runs.first().and_then(|r| r.error_message.as_deref());

        match dead_letter::insert_dead_letter(
            pool,
            &dlq_id,
            &task.id,
            &task.queue_name,
            &task.task_name,
            task.input.as_ref(),
            error_message,
            task.attempt_count,
            &task.metadata,
        )
        .await
        {
            Ok(_) => {
                if let Err(e) = tasks::move_to_dead_letter(pool, &task.id).await {
                    error!(task_id = %task.id, error = %e, "Failed to update task status to DEAD_LETTER");
                }
                info!(task_id = %task.id, "Moved to dead letter queue");
                valka_core::metrics::record_task_dead_lettered(&task.queue_name);
            }
            Err(e) => {
                error!(task_id = %task.id, error = %e, "Failed to insert into DLQ");
            }
        }
    }

    Ok(count)
}
