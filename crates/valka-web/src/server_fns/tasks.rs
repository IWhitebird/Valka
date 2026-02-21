use leptos::prelude::*;

use crate::api::types::{CreateTaskRequest, Task, TaskLog, TaskRun};

#[server]
pub async fn list_tasks(
    queue_name: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Task>, ServerFnError> {
    let mut params = Vec::new();
    if let Some(q) = &queue_name {
        if !q.is_empty() {
            params.push(format!("queue_name={q}"));
        }
    }
    if let Some(s) = &status {
        if !s.is_empty() {
            params.push(format!("status={s}"));
        }
    }
    if let Some(l) = limit {
        params.push(format!("limit={l}"));
    }
    if let Some(o) = offset {
        params.push(format!("offset={o}"));
    }
    let query = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    crate::api::client::get(&format!("/api/v1/tasks{query}")).await
}

#[server]
pub async fn get_task(task_id: String) -> Result<Task, ServerFnError> {
    crate::api::client::get(&format!("/api/v1/tasks/{task_id}")).await
}

#[server]
pub async fn create_task(req: CreateTaskRequest) -> Result<Task, ServerFnError> {
    crate::api::client::post("/api/v1/tasks", &req).await
}

#[server]
pub async fn cancel_task(task_id: String) -> Result<Task, ServerFnError> {
    crate::api::client::post_empty(&format!("/api/v1/tasks/{task_id}/cancel")).await
}

#[server]
pub async fn delete_task(task_id: String) -> Result<serde_json::Value, ServerFnError> {
    crate::api::client::delete(&format!("/api/v1/tasks/{task_id}")).await
}

#[server]
pub async fn clear_all_tasks() -> Result<serde_json::Value, ServerFnError> {
    crate::api::client::delete("/api/v1/tasks").await
}

#[server]
pub async fn get_task_runs(task_id: String) -> Result<Vec<TaskRun>, ServerFnError> {
    crate::api::client::get(&format!("/api/v1/tasks/{task_id}/runs")).await
}

#[server]
pub async fn get_run_logs(
    task_id: String,
    run_id: String,
) -> Result<Vec<TaskLog>, ServerFnError> {
    crate::api::client::get(&format!("/api/v1/tasks/{task_id}/runs/{run_id}/logs")).await
}
