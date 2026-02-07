//! Basic producer example: creates tasks on a Valka server.
//!
//! Usage:
//!   cargo run -p valka-examples --example producer
//!
//! Requires a running Valka server at http://127.0.0.1:50051

use valka_sdk::ValkaClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the Valka server
    let mut client = ValkaClient::connect("http://127.0.0.1:50051").await?;
    println!("Connected to Valka server");

    // Create a task with JSON input
    let task = client
        .create_task(
            "emails",
            "send-welcome",
            Some(serde_json::json!({
                "to": "user@example.com",
                "subject": "Welcome!",
                "body": "Thanks for signing up."
            })),
        )
        .await?;

    println!("Created task: {}", task.id);
    println!("  Queue:  {}", task.queue_name);
    println!("  Status: {}", task.status);

    // Retrieve the task by ID
    let fetched = client.get_task(&task.id).await?;
    println!("\nFetched task: {} (status={})", fetched.id, fetched.status);

    // List tasks in the "emails" queue
    let tasks = client.list_tasks(Some("emails"), None, 10).await?;
    println!("\nTasks in 'emails' queue: {}", tasks.len());
    for t in &tasks {
        println!("  {} - {} (status={})", t.id, t.task_name, t.status);
    }

    Ok(())
}
