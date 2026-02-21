use reqwest::Client;
use server_fn::ServerFnError;
use std::sync::LazyLock;

static CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

pub fn api_base() -> String {
    std::env::var("VALKA_API_URL").unwrap_or_else(|_| "http://localhost:8989".to_string())
}

pub async fn get<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, ServerFnError> {
    let url = format!("{}{}", api_base(), path);
    let resp = CLIENT
        .get(&url)
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Request failed: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(ServerFnError::new(format!("API {status}: {body}")));
    }
    resp.json()
        .await
        .map_err(|e| ServerFnError::new(format!("JSON decode: {e}")))
}

pub async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
    path: &str,
    body: &B,
) -> Result<T, ServerFnError> {
    let url = format!("{}{}", api_base(), path);
    let resp = CLIENT
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Request failed: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(ServerFnError::new(format!("API {status}: {body}")));
    }
    resp.json()
        .await
        .map_err(|e| ServerFnError::new(format!("JSON decode: {e}")))
}

pub async fn post_empty<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, ServerFnError> {
    let url = format!("{}{}", api_base(), path);
    let resp = CLIENT
        .post(&url)
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Request failed: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(ServerFnError::new(format!("API {status}: {body}")));
    }
    resp.json()
        .await
        .map_err(|e| ServerFnError::new(format!("JSON decode: {e}")))
}

pub async fn delete<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, ServerFnError> {
    let url = format!("{}{}", api_base(), path);
    let resp = CLIENT
        .delete(&url)
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Request failed: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(ServerFnError::new(format!("API {status}: {body}")));
    }
    resp.json()
        .await
        .map_err(|e| ServerFnError::new(format!("JSON decode: {e}")))
}
