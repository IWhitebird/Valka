//! Queue configuration: `queues/{hh}/{queue}`. Created on first use with the default
//! partition count (create-if-absent), changed only by an admin call (CAS).

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::WalError;
use crate::lsn::keys;
use crate::store::{CasOutcome, Store};

/// The partition count before the last change, kept while keyed creates may still find
/// a key's task under its old partition (idempotency across a resize).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousPartitions {
    pub partitions: u32,
    pub changed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueConfig {
    pub queue: String,
    pub partitions: u32,
    #[serde(default)]
    pub previous: Option<PreviousPartitions>,
    pub version: u64,
    pub created_at: DateTime<Utc>,
}

/// Read a queue's config, creating it with `default_partitions` if it does not exist.
pub async fn get_or_create(
    store: &Store,
    queue: &str,
    default_partitions: u32,
) -> Result<QueueConfig, WalError> {
    let key = keys::queue(queue);
    loop {
        if let Some((bytes, _)) = store.get(&key).await? {
            return serde_json::from_slice(&bytes)
                .map_err(|e| WalError::Corrupt(format!("{key}: {e}")));
        }
        let cfg = QueueConfig {
            queue: queue.to_string(),
            partitions: default_partitions.max(1),
            previous: None,
            version: 1,
            created_at: Utc::now(),
        };
        if store
            .put_create(&key, Bytes::from(serde_json::to_vec(&cfg)?))
            .await?
            .is_some()
        {
            return Ok(cfg);
        }
    }
}

/// Change a queue's partition count. Returns the new config.
pub async fn set_partitions(
    store: &Store,
    queue: &str,
    partitions: u32,
    default_partitions: u32,
) -> Result<QueueConfig, WalError> {
    if partitions == 0 || partitions > 4096 {
        return Err(WalError::Config(format!(
            "partitions = {partitions}: must be 1..=4096"
        )));
    }
    let key = keys::queue(queue);
    loop {
        let Some((bytes, etag)) = store.get(&key).await? else {
            get_or_create(store, queue, default_partitions).await?;
            continue;
        };
        let cur: QueueConfig =
            serde_json::from_slice(&bytes).map_err(|e| WalError::Corrupt(format!("{key}: {e}")))?;
        if cur.partitions == partitions {
            return Ok(cur);
        }
        let Some(etag) = etag.or(store.head_etag(&key).await?.flatten()) else {
            return Err(WalError::Config("storage backend returns no ETags".into()));
        };
        let next = QueueConfig {
            partitions,
            previous: Some(PreviousPartitions {
                partitions: cur.partitions,
                changed_at: Utc::now(),
            }),
            version: cur.version + 1,
            ..cur
        };
        if let CasOutcome::Written(_) = store
            .put_if_match(&key, &etag, Bytes::from(serde_json::to_vec(&next)?))
            .await?
        {
            return Ok(next);
        }
    }
}

/// Every queue config in the bucket.
pub async fn list(store: &Store) -> Result<Vec<QueueConfig>, WalError> {
    let mut out = Vec::new();
    for (key, _) in store.list(keys::QUEUES_PREFIX).await? {
        if let Some((bytes, _)) = store.get(&key).await?
            && let Ok(c) = serde_json::from_slice::<QueueConfig>(&bytes)
        {
            out.push(c);
        }
    }
    out.sort_by(|a, b| a.queue.cmp(&b.queue));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn created_once_and_resized_with_history() {
        let s = Store::memory();
        let a = get_or_create(&s, "emails", 16).await.unwrap();
        assert_eq!(a.partitions, 16);
        let b = get_or_create(&s, "emails", 4).await.unwrap();
        assert_eq!(b.partitions, 16, "the first writer's default sticks");
        let c = set_partitions(&s, "emails", 32, 16).await.unwrap();
        assert_eq!(c.partitions, 32);
        assert_eq!(c.version, 2);
        assert_eq!(c.previous.unwrap().partitions, 16);
        assert!(set_partitions(&s, "emails", 0, 16).await.is_err());
        assert_eq!(list(&s).await.unwrap().len(), 1);
    }
}
