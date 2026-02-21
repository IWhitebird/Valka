use leptos::prelude::*;

use crate::api::types::DeadLetter;

#[server]
pub async fn list_dead_letters(
    queue_name: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<DeadLetter>, ServerFnError> {
    let mut params = Vec::new();
    if let Some(q) = &queue_name {
        if !q.is_empty() {
            params.push(format!("queue_name={q}"));
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
    crate::api::client::get(&format!("/api/v1/dead-letters{query}")).await
}
