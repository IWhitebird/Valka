//! Signal demo: creates a task with initial values, worker prints them in a loop,
//! producer sends update signals every 2s for ~15s, then sends a stop signal.
//!
//! Usage:
//!   cargo run -p valka-examples --example signal_demo

use std::collections::HashMap;

use tokio::time::{Duration, timeout};
use valka_sdk::{TaskContext, ValkaClient, ValkaWorker};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let mut client = ValkaClient::connect("http://127.0.0.1:50051").await?;

    // Create task with initial a=4, b=5
    let task = client
        .create_task(
            "experiments",
            "signal-demo",
            Some(serde_json::json!({"a": 4, "b": 5})),
        )
        .await?;
    println!("Created task {}", task.id);

    // Start worker in background
    let worker = ValkaWorker::builder()
        .name("signal-demo-worker")
        .server_addr("http://127.0.0.1:50051")
        .queues(&["experiments"])
        .handler(handle_task)
        .build()
        .await?;

    tokio::spawn(async move {
        if let Err(e) = worker.run().await {
            eprintln!("Worker error: {e}");
        }
    });

    // Give worker time to connect and pick up the task
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Send update signals every 2 seconds for ~14 seconds, incrementing a and b by 1
    let (mut a, mut b) = (4i64, 5i64);
    for _ in 0..7 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        a += 1;
        b += 1;
        let (_sig_id, delivered) = client
            .send_signal(
                &task.id,
                "update",
                Some(serde_json::json!({"a": a, "b": b})),
            )
            .await?;
        println!("[producer] Sent update a={a} b={b} (delivered={delivered})");
    }

    // Send stop signal
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (_, delivered) = client.send_signal(&task.id, "stop", None).await?;
    println!("[producer] Sent stop signal (delivered={delivered})");

    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("Done.");
    Ok(())
}

async fn handle_task(mut ctx: TaskContext) -> Result<serde_json::Value, String> {
    let input: HashMap<String, f64> = ctx.input().map_err(|e| e.to_string())?;
    let mut a = input["a"] as i64;
    let mut b = input["b"] as i64;

    ctx.log(&format!("Started with a={a}, b={b}")).await;

    loop {
        // Wait up to 1 second for a signal; if none arrives, print current values
        match timeout(Duration::from_secs(1), ctx.receive_signal()).await {
            Ok(Some(sig)) => match sig.name.as_str() {
                "update" => {
                    let payload: HashMap<String, f64> =
                        sig.parse_payload().map_err(|e| e.to_string())?;
                    a = payload["a"] as i64;
                    b = payload["b"] as i64;
                    ctx.log(&format!("Updated: a={a}, b={b}")).await;
                }
                "stop" => {
                    ctx.log("Received stop signal").await;
                    break;
                }
                _ => {}
            },
            Ok(None) => break, // channel closed
            Err(_) => {
                // Timeout — no signal, print current values
                ctx.log(&format!("a={a}  b={b}  sum={}", a + b)).await;
            }
        }
    }

    ctx.log(&format!("Final: a={a}  b={b}  sum={}", a + b))
        .await;
    Ok(serde_json::json!({"status": "stopped gracefully"}))
}
