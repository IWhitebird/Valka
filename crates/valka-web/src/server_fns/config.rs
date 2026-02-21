use leptos::prelude::*;

#[server]
pub async fn get_api_url() -> Result<String, ServerFnError> {
    Ok(crate::api::client::api_base())
}
