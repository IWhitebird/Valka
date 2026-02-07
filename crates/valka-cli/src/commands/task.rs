use anyhow::Result;
use tonic::transport::Channel;
use valka_proto::api_service_client::ApiServiceClient;
use valka_proto::*;

pub async fn create(
    server: &str,
    queue: &str,
    name: &str,
    input: Option<String>,
    priority: i32,
    max_retries: i32,
    timeout: i32,
) -> Result<()> {
    let mut client = connect(server).await?;

    let response = client
        .create_task(CreateTaskRequest {
            queue_name: queue.to_string(),
            task_name: name.to_string(),
            input: input.unwrap_or_default(),
            priority,
            max_retries,
            timeout_seconds: timeout,
            idempotency_key: String::new(),
            metadata: String::new(),
            scheduled_at: String::new(),
        })
        .await?;

    if let Some(task) = response.into_inner().task {
        println!("Task created:");
        print_task(&task);
    }

    Ok(())
}

pub async fn get(server: &str, task_id: &str) -> Result<()> {
    let mut client = connect(server).await?;

    let response = client
        .get_task(GetTaskRequest {
            task_id: task_id.to_string(),
        })
        .await?;

    if let Some(task) = response.into_inner().task {
        print_task(&task);
    } else {
        println!("Task not found");
    }

    Ok(())
}

pub async fn list(
    server: &str,
    queue: Option<String>,
    status: Option<String>,
    limit: i32,
) -> Result<()> {
    let mut client = connect(server).await?;

    let status_enum = status.as_deref().map(status_str_to_proto).unwrap_or(0);

    let response = client
        .list_tasks(ListTasksRequest {
            queue_name: queue.unwrap_or_default(),
            status: status_enum,
            pagination: Some(Pagination {
                page_size: limit,
                page_token: String::new(),
            }),
        })
        .await?;

    let tasks = response.into_inner().tasks;
    if tasks.is_empty() {
        println!("No tasks found");
        return Ok(());
    }

    println!(
        "{:<38} {:<20} {:<20} {:<12} {:<8}",
        "ID", "QUEUE", "NAME", "STATUS", "ATTEMPT"
    );
    println!("{}", "-".repeat(98));

    for task in tasks {
        println!(
            "{:<38} {:<20} {:<20} {:<12} {:<8}",
            task.id,
            task.queue_name,
            task.task_name,
            proto_status_to_str(task.status),
            task.attempt_count,
        );
    }

    Ok(())
}

pub async fn cancel(server: &str, task_id: &str) -> Result<()> {
    let mut client = connect(server).await?;

    let response = client
        .cancel_task(CancelTaskRequest {
            task_id: task_id.to_string(),
        })
        .await?;

    if let Some(task) = response.into_inner().task {
        println!("Task cancelled:");
        print_task(&task);
    }

    Ok(())
}

async fn connect(server: &str) -> Result<ApiServiceClient<Channel>> {
    let channel = Channel::from_shared(server.to_string())?.connect().await?;
    Ok(ApiServiceClient::new(channel))
}

fn print_task(task: &TaskMeta) {
    println!("  ID:             {}", task.id);
    println!("  Queue:          {}", task.queue_name);
    println!("  Name:           {}", task.task_name);
    println!("  Status:         {}", proto_status_to_str(task.status));
    println!("  Priority:       {}", task.priority);
    println!(
        "  Attempt:        {}/{}",
        task.attempt_count, task.max_retries
    );
    println!("  Timeout:        {}s", task.timeout_seconds);
    if !task.input.is_empty() {
        println!("  Input:          {}", task.input);
    }
    if !task.output.is_empty() {
        println!("  Output:         {}", task.output);
    }
    if !task.error_message.is_empty() {
        println!("  Error:          {}", task.error_message);
    }
    println!("  Created:        {}", task.created_at);
    println!("  Updated:        {}", task.updated_at);
}

fn status_str_to_proto(s: &str) -> i32 {
    match s.to_uppercase().as_str() {
        "PENDING" => 1,
        "DISPATCHING" => 2,
        "RUNNING" => 3,
        "COMPLETED" => 4,
        "FAILED" => 5,
        "RETRY" => 6,
        "DEAD_LETTER" => 7,
        "CANCELLED" => 8,
        _ => 0,
    }
}

fn proto_status_to_str(status: i32) -> &'static str {
    match status {
        1 => "PENDING",
        2 => "DISPATCHING",
        3 => "RUNNING",
        4 => "COMPLETED",
        5 => "FAILED",
        6 => "RETRY",
        7 => "DEAD_LETTER",
        8 => "CANCELLED",
        _ => "UNKNOWN",
    }
}
