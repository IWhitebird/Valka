pub mod cluster;
pub mod logs;
pub mod task;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Where and how to reach the cluster.
pub struct Conn {
    /// gRPC addresses (task and log commands).
    pub servers: Vec<String>,
    /// REST base URL (cluster, worker and queue commands).
    pub http: String,
    pub api_key: Option<String>,
    pub admin_token: Option<String>,
}

/// Which credential a REST call needs.
pub enum Auth {
    Api,
    Admin,
}

impl Conn {
    pub async fn client(&self) -> Result<valka_sdk::ValkaClient> {
        let mut b = valka_sdk::ValkaClient::builder().addrs(&self.servers);
        if let Some(k) = &self.api_key {
            b = b.api_key(k);
        }
        b.connect()
            .await
            .with_context(|| format!("cannot reach {}", self.servers.join(", ")))
    }

    /// One REST call; the error carries the server's message.
    pub async fn rest(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        auth: Auth,
    ) -> Result<Value> {
        let url = format!("{}{path}", self.http);
        let mut req = reqwest::Client::new().request(method, &url);
        let token = match auth {
            Auth::Api => self.api_key.as_deref(),
            Auth::Admin => match self.admin_token.as_deref() {
                Some(t) => Some(t),
                None => {
                    bail!("this command needs the admin token: --admin-token or VALKA_ADMIN_TOKEN")
                }
            },
        };
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("cannot reach {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or(text);
            bail!("{status}: {msg}");
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }
}
