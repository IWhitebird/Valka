//! Basic worker example: processes tasks from the "emails" queue.
//!
//! Usage:
//!   cargo run -p valka-examples --example worker
//!
//! Requires a running Valka server at http://127.0.0.1:50051.
//! Run the producer example to enqueue tasks.

use valka_sdk::{TaskContext, ValkaWorker};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let worker = ValkaWorker::builder()
        .name("example-worker")
        .server_addr("http://127.0.0.1:50051")
        .queues(&["emails"])
        .concurrency(4)
        .handler(handle_task)
        .build()
        .await?;

    println!("Worker starting... press Ctrl+C to stop.");
    worker.run().await?;

    Ok(())
}

async fn handle_task(ctx: TaskContext) -> Result<serde_json::Value, String> {
    ctx.log(&format!(
        "Processing task '{}' (attempt {})",
        ctx.task_name, ctx.attempt_number
    ))
    .await;

    // Parse the input JSON
    let input: serde_json::Value = ctx
        .input()
        .map_err(|e| format!("Failed to parse input: {e}"))?;

    let to = input
        .get("to")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    ctx.log(&format!("Sending email to {to}...")).await;
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    ctx.log("Email sent successfully").await;

    Ok(serde_json::json!({
        "delivered_to": to,
        "status": "sent"
    }))
}
