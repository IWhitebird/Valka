use leptos::prelude::*;

use crate::api::types::Worker;

#[server]
pub async fn list_workers() -> Result<Vec<Worker>, ServerFnError> {
    crate::api::client::get("/api/v1/workers").await
}
