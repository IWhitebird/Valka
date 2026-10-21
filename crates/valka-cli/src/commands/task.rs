use anyhow::{Result, bail};
use valka_proto::*;

use super::Conn;

/// The options of `valka task create`.
pub struct NewTask {
    pub queue: String,
    pub name: String,
    pub input: Option<String>,
    pub priority: i32,
    pub max_retries: i32,
    pub timeout: i32,
    pub idempotency_key: Option<String>,
    pub scheduled_at: Option<String>,
}

pub async fn create(conn: &Conn, t: NewTask, quiet: bool) -> Result<()> {
    if let Some(input) = &t.input {
        serde_json::from_str::<serde_json::Value>(input)
            .map_err(|e| anyhow::anyhow!("--input is not valid JSON: {e}"))?;
    }
    let mut client = conn.client().await?;
    let task = client
        .create_task_request(CreateTaskRequest {
            queue_name: t.queue,
            task_name: t.name,
            input: t.input.unwrap_or_default(),
            priority: t.priority,
            max_retries: t.max_retries,
            timeout_seconds: t.timeout,
            idempotency_key: t.idempotency_key.unwrap_or_default(),
            metadata: String::new(),
            scheduled_at: t.scheduled_at.unwrap_or_default(),
        })
        .await?;
    if quiet {
        println!("{}", task.id);
    } else {
        println!("Task created");
        print_task(&task);
    }
    Ok(())
}

pub async fn get(conn: &Conn, task_id: &str) -> Result<()> {
    let mut client = conn.client().await?;
    print_task(&client.get_task(task_id).await?);
    Ok(())
}

pub async fn list(
    conn: &Conn,
    queue: Option<String>,
    status: Option<String>,
    limit: i32,
) -> Result<()> {
    let status = match status.as_deref() {
        Some(s) => Some(status_from_str(s)?),
        None => None,
    };
    let mut client = conn.client().await?;
    let tasks = client.list_tasks(queue.as_deref(), status, limit).await?;
    if tasks.is_empty() {
        println!("No tasks");
        return Ok(());
    }
    println!(
        "{:<38} {:<18} {:<20} {:<12} {:>7}",
        "ID", "QUEUE", "NAME", "STATUS", "ATTEMPT"
    );
    for task in tasks {
        println!(
            "{:<38} {:<18} {:<20} {:<12} {:>7}",
            task.id,
            truncate(&task.queue_name, 18),
            truncate(&task.task_name, 20),
            status_to_str(task.status),
            task.attempt_count,
        );
    }
    Ok(())
}

pub async fn cancel(conn: &Conn, task_id: &str) -> Result<()> {
    let mut client = conn.client().await?;
    let task = client.cancel_task(task_id).await?;
    println!("Task cancelled");
    print_task(&task);
    Ok(())
}

fn print_task(task: &TaskMeta) {
    let row = |k: &str, v: &str| println!("  {k:<16}{v}");
    row("ID", &task.id);
    row("Queue", &task.queue_name);
    row("Name", &task.task_name);
    row("Status", status_to_str(task.status));
    row("Priority", &task.priority.to_string());
    row(
        "Attempts",
        &format!("{} of {}", task.attempt_count, task.max_retries),
    );
    row("Timeout", &format!("{} s", task.timeout_seconds));
    if !task.idempotency_key.is_empty() {
        row("Idempotency key", &task.idempotency_key);
    }
    if !task.scheduled_at.is_empty() {
        row("Scheduled at", &task.scheduled_at);
    }
    if !task.input.is_empty() {
        row("Input", &task.input);
    }
    if !task.output.is_empty() {
        row("Output", &task.output);
    }
    if !task.error_message.is_empty() {
        row("Error", &task.error_message);
    }
    row("Created", &task.created_at);
    row("Updated", &task.updated_at);
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n - 1).collect();
        t.push('…');
        t
    }
}

fn status_from_str(s: &str) -> Result<i32> {
    Ok(match s.to_uppercase().as_str() {
        "PENDING" => 1,
        "RUNNING" => 3,
        "COMPLETED" => 4,
        "FAILED" => 5,
        "RETRY" => 6,
        "DEAD_LETTER" => 7,
        "CANCELLED" => 8,
        other => bail!(
            "unknown status {other}: use PENDING, RUNNING, COMPLETED, FAILED, RETRY, \
             DEAD_LETTER or CANCELLED"
        ),
    })
}

fn status_to_str(status: i32) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip() {
        for s in [
            "PENDING",
            "RUNNING",
            "COMPLETED",
            "FAILED",
            "RETRY",
            "DEAD_LETTER",
            "CANCELLED",
        ] {
            assert_eq!(status_to_str(status_from_str(s).unwrap()), s);
        }
        assert_eq!(status_from_str("running").unwrap(), 3);
        assert!(status_from_str("DONE").is_err());
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
