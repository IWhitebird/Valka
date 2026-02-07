//! Full lifecycle example: creates tasks and processes them in a single binary.
//!
//! Usage:
//!   cargo run -p valka-examples --example full_lifecycle
//!
//! Requires a running Valka server at http://127.0.0.1:50051.

use valka_sdk::{TaskContext, ValkaClient, ValkaWorker};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let server_addr = "http://127.0.0.1:50051";

    // Start the worker in a background task
    let worker = ValkaWorker::builder()
        .name("lifecycle-worker")
        .server_addr(server_addr)
        .queues(&["demo"])
        .concurrency(2)
        .handler(|ctx: TaskContext| async move {
            ctx.log(&format!("Handling: {}", ctx.task_name)).await;

            let input: serde_json::Value = ctx
                .input()
                .map_err(|e| format!("bad input: {e}"))?;

            let n = input.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
            let result = n * 2;

            ctx.log(&format!("Computed {n} * 2 = {result}")).await;

            Ok(serde_json::json!({ "result": result }))
        })
        .build()
        .await?;

    let shutdown = worker.shutdown_handle();
    let worker_handle = tokio::spawn(async move {
        if let Err(e) = worker.run().await {
            eprintln!("Worker error: {e}");
        }
    });

    // Give the worker a moment to connect
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Create some tasks
    let mut client = ValkaClient::connect(server_addr).await?;

    println!("Creating 5 tasks...");
    let mut task_ids = Vec::new();
    for i in 1..=5 {
        let task = client
            .create_task("demo", "multiply", Some(serde_json::json!({ "n": i })))
            .await?;
        println!("  Created task {} with n={i}", task.id);
        task_ids.push(task.id);
    }

    // Poll until all created tasks complete (or timeout)
    println!("\nWaiting for tasks to complete...");
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(30);
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let mut completed = 0;
        for id in &task_ids {
            let task = client.get_task(id).await?;
            // 4 = COMPLETED, 5 = FAILED
            if task.status == 4 || task.status == 5 {
                completed += 1;
            }
        }

        let total = task_ids.len();
        println!("  {completed}/{total} tasks completed");

        if completed >= total || tokio::time::Instant::now() > deadline {
            break;
        }
    }

    // Print final status
    println!("\nFinal task statuses:");
    for id in &task_ids {
        let task = client.get_task(id).await?;
        println!("  {} - status={} output={}", task.id, task.status, task.output);
    }

    println!("\nShutting down worker...");
    shutdown.shutdown();
    let _ = worker_handle.await;
    println!("Done!");

    Ok(())
}
