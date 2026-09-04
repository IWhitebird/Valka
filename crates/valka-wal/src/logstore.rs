//! Task log chunks: `logs/{run_id}/{uuidv7}.log`, zstd JSON lines. Append-only per run;
//! chunk ids are time-ordered so a prefix list returns them in order.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::WalError;
use crate::lsn::keys;
use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub task_run_id: String,
    pub timestamp_ms: i64,
    pub level: String,
    pub message: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

pub async fn append_chunk(store: &Store, run_id: &str, lines: &[LogLine]) -> Result<(), WalError> {
    if lines.is_empty() {
        return Ok(());
    }
    let mut buf = Vec::with_capacity(lines.len() * 128);
    for l in lines {
        serde_json::to_writer(&mut buf, l)?;
        buf.push(b'\n');
    }
    let body = zstd::encode_all(&buf[..], 1)?;
    let chunk_id = uuid::Uuid::now_v7().to_string();
    store
        .put(&keys::log_chunk(run_id, &chunk_id), Bytes::from(body))
        .await?;
    Ok(())
}

/// All log lines for a run, in write order. `limit` caps the result.
pub async fn read_run(store: &Store, run_id: &str, limit: usize) -> Result<Vec<LogLine>, WalError> {
    let chunks = store.list(&keys::logs_prefix(run_id)).await?;
    let mut out = Vec::new();
    for (key, _) in chunks {
        let Some((bytes, _)) = store.get(&key).await? else {
            continue;
        };
        let raw = zstd::decode_all(&bytes[..])?;
        for line in raw.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            match serde_json::from_slice::<LogLine>(line) {
                Ok(l) => out.push(l),
                Err(e) => tracing::warn!(key, error = %e, "skipping corrupt log line"),
            }
            if out.len() >= limit {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

pub async fn delete_run(store: &Store, run_id: &str) -> Result<(), WalError> {
    for (key, _) in store.list(&keys::logs_prefix(run_id)).await? {
        store.delete(&key).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(i: i64) -> LogLine {
        LogLine {
            task_run_id: "r".into(),
            timestamp_ms: i,
            level: "INFO".into(),
            message: format!("m{i}"),
            metadata: None,
        }
    }

    #[tokio::test]
    async fn chunks_read_back_in_order() {
        let s = Store::memory();
        append_chunk(&s, "r", &[line(1), line(2)]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        append_chunk(&s, "r", &[line(3)]).await.unwrap();
        let all = read_run(&s, "r", 100).await.unwrap();
        assert_eq!(
            all.iter().map(|l| l.timestamp_ms).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(read_run(&s, "r", 2).await.unwrap().len(), 2);
        delete_run(&s, "r").await.unwrap();
        assert!(read_run(&s, "r", 100).await.unwrap().is_empty());
    }
}
