use leptos::prelude::*;

use crate::api::types::{SendSignalRequest, SendSignalResponse, TaskSignal};

#[server]
pub async fn list_signals(task_id: String) -> Result<Vec<TaskSignal>, ServerFnError> {
    crate::api::client::get(&format!("/api/v1/tasks/{task_id}/signals")).await
}

#[server]
pub async fn send_signal(
    task_id: String,
    req: SendSignalRequest,
) -> Result<SendSignalResponse, ServerFnError> {
    crate::api::client::post(&format!("/api/v1/tasks/{task_id}/signal"), &req).await
}
