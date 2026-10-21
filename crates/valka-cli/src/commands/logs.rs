use anyhow::{Context, Result};
use futures::StreamExt;
use valka_proto::api_service_client::ApiServiceClient;
use valka_proto::*;
use valka_sdk::transport::{ApiKeyAuth, connect_channel};

use super::Conn;

pub async fn tail(conn: &Conn, task_run_id: &str) -> Result<()> {
    let auth = ApiKeyAuth::new(conn.api_key.as_deref())?;
    let mut channel = None;
    for addr in &conn.servers {
        if let Ok(c) = connect_channel(addr, std::time::Duration::from_secs(5)).await {
            channel = Some(c);
            break;
        }
    }
    let channel = channel.with_context(|| format!("cannot reach {}", conn.servers.join(", ")))?;
    let mut client = ApiServiceClient::with_interceptor(channel, auth);
    let mut stream = client
        .subscribe_logs(SubscribeLogsRequest {
            task_run_id: task_run_id.to_string(),
            include_history: true,
        })
        .await?
        .into_inner();

    eprintln!("Following logs of run {task_run_id} (Ctrl-C to stop)");
    while let Some(entry) = stream.next().await {
        let log = entry?;
        let level = match log.level {
            1 => "DEBUG",
            2 => "INFO",
            3 => "WARN",
            4 => "ERROR",
            _ => "-",
        };
        let ts = chrono::DateTime::from_timestamp_millis(log.timestamp_ms)
            .map(|t| t.format("%H:%M:%S%.3f").to_string())
            .unwrap_or_else(|| "--:--:--.---".to_string());
        println!("{ts} {level:<5} {}", log.message);
    }
    Ok(())
}
