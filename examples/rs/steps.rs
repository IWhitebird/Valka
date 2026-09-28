//! Step checkpoints: a three-step pipeline whose `transform` step fails on the first attempt.
//! The retry skips the checkpointed `fetch` step and resumes at `transform`.
//!
//! Usage:
//!   cargo run -p valka-examples --example steps
//!
//! Requires a running Valka server at http://127.0.0.1:50051. Enqueue work with:
//!   curl -X POST localhost:8989/api/v1/tasks -H 'content-type: application/json' \
//!     -d '{"queue_name":"pipeline","task_name":"etl"}'

use valka_sdk::{TaskContext, ValkaWorker};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let worker = ValkaWorker::builder()
        .name("steps-worker")
        .server_addr("http://127.0.0.1:50051")
        .queues(&["pipeline"])
        .handler(handle_task)
        .build()
        .await?;

    worker.run().await?;
    Ok(())
}

async fn handle_task(ctx: TaskContext) -> Result<serde_json::Value, String> {
    let attempt = ctx.attempt_number;
    let rows: u32 = ctx
        .step("fetch", || async {
            println!("fetch runs on attempt {attempt}");
            Ok::<_, String>(3)
        })
        .await?;
    let doubled: u32 = ctx
        .step("transform", || async move {
            if attempt == 1 {
                return Err("transform failed; the retry resumes here".to_string());
            }
            Ok(rows * 2)
        })
        .await?;
    let stored: String = ctx
        .step("store", || async move {
            Ok::<_, String>(format!("stored {doubled} rows"))
        })
        .await?;
    Ok(
        serde_json::json!({ "rows": rows, "doubled": doubled, "stored": stored, "attempt": attempt }),
    )
}
