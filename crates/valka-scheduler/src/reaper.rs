use sqlx::PgPool;
use tracing::{error, info, warn};
use valka_db::queries::{dead_letter, task_runs, tasks};

/// Scan for expired leases and handle them:
/// - If task can retry: set status to RETRY
/// - If max retries exceeded: move to DLQ
pub async fn reap_expired_leases(pool: &PgPool) -> Result<usize, sqlx::Error> {
    let expired = task_runs::find_expired_leases(pool).await?;
    let count = expired.len();

    for run in expired {
        // Fail the run
        if let Err(e) = task_runs::fail_task_run(pool, &run.id, "Lease expired").await {
            error!(run_id = %run.id, error = %e, "Failed to fail expired run");
            continue;
        }

        // Check if the task can retry
        let task = tasks::get_task(pool, &run.task_id).await?;
        if let Some(task) = task {
            if task.attempt_count < task.max_retries {
                // Schedule retry
                if let Err(e) = tasks::update_task_status(pool, &task.id, "RETRY").await {
                    error!(task_id = %task.id, error = %e, "Failed to set task to RETRY");
                }
                info!(task_id = %task.id, "Expired lease - scheduling retry");
            } else {
                // Move to dead letter — insert DLQ entry first, then update status
                let dlq_id = uuid::Uuid::now_v7().to_string();
                let runs =
                    task_runs::get_runs_for_task(pool, &task.id).await.unwrap_or_default();
                let error_message = runs.first().and_then(|r| r.error_message.as_deref());

                if let Err(e) = dead_letter::insert_dead_letter(
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
                    error!(task_id = %task.id, error = %e, "Failed to insert DLQ entry");
                }

                if let Err(e) = tasks::move_to_dead_letter(pool, &task.id).await {
                    error!(task_id = %task.id, error = %e, "Failed to move task to DLQ");
                }
                valka_core::metrics::record_task_dead_lettered(&task.queue_name);
                warn!(task_id = %task.id, "Expired lease - moved to DLQ (max retries exceeded)");
            }
        }
    }

    if count > 0 {
        info!(count, "Reaped expired leases");
    }

    Ok(count)
}
