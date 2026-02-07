use anyhow::Result;
use futures::StreamExt;
use tonic::transport::Channel;
use valka_proto::api_service_client::ApiServiceClient;
use valka_proto::*;

pub async fn tail(server: &str, task_run_id: &str) -> Result<()> {
    let channel = Channel::from_shared(server.to_string())?.connect().await?;
    let mut client = ApiServiceClient::new(channel);

    let response = client
        .subscribe_logs(SubscribeLogsRequest {
            task_run_id: task_run_id.to_string(),
            include_history: true,
        })
        .await?;

    let mut stream = response.into_inner();

    println!("Tailing logs for task run: {task_run_id}");
    println!("{}", "-".repeat(80));

    while let Some(entry) = stream.next().await {
        match entry {
            Ok(log) => {
                let level = match log.level {
                    1 => "DEBUG",
                    2 => "INFO",
                    3 => "WARN",
                    4 => "ERROR",
                    _ => "???",
                };
                let ts = chrono::DateTime::from_timestamp_millis(log.timestamp_ms)
                    .map(|t| t.format("%H:%M:%S%.3f").to_string())
                    .unwrap_or_else(|| "??:??:??.???".to_string());
                println!("{ts} [{level:<5}] {}", log.message);
            }
            Err(e) => {
                eprintln!("Stream error: {e}");
                break;
            }
        }
    }

    Ok(())
}
