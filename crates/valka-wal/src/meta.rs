//! `cluster.json` and the storage self-test.

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;
use valka_core::{DEFAULT_NUM_SHARDS, valid_shard_count};

use crate::error::WalError;
use crate::lsn::keys;
use crate::store::{CasOutcome, Store};

/// Layout version of everything in the bucket. A node refuses a bucket with another one.
pub const FORMAT: u32 = 2;

/// Created once, by the first node to start on a bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterMeta {
    pub cluster_id: String,
    pub num_shards: u32,
    pub format: u32,
    pub created_at: DateTime<Utc>,
}

/// Read `cluster.json`, creating it on a new bucket. `num_shards = 0` means "use the
/// bucket's count, or the default for a new bucket"; anything else must match.
pub async fn ensure(store: &Store, num_shards: u32) -> Result<ClusterMeta, WalError> {
    if num_shards != 0 && !valid_shard_count(num_shards) {
        return Err(WalError::Config(format!(
            "num_shards = {num_shards}: must be a power of two in 16..=65536"
        )));
    }
    loop {
        if let Some((bytes, _)) = store.get(keys::CLUSTER).await? {
            let meta: ClusterMeta = serde_json::from_slice(&bytes)
                .map_err(|e| WalError::Corrupt(format!("cluster.json: {e}")))?;
            if meta.format != FORMAT {
                return Err(WalError::Config(format!(
                    "bucket has format {} but this build reads format {FORMAT}; \
                     use a new bucket (or prefix)",
                    meta.format
                )));
            }
            if num_shards != 0 && meta.num_shards != num_shards {
                return Err(WalError::Config(format!(
                    "configured num_shards = {num_shards} but the cluster was created with {}; \
                     the shard count of a cluster never changes",
                    meta.num_shards
                )));
            }
            if !valid_shard_count(meta.num_shards) {
                return Err(WalError::Corrupt(format!(
                    "cluster.json has num_shards = {}",
                    meta.num_shards
                )));
            }
            return Ok(meta);
        }
        let meta = ClusterMeta {
            cluster_id: uuid::Uuid::now_v7().to_string(),
            num_shards: if num_shards == 0 {
                DEFAULT_NUM_SHARDS
            } else {
                num_shards
            },
            format: FORMAT,
            created_at: Utc::now(),
        };
        let body = Bytes::from(serde_json::to_vec_pretty(&meta)?);
        if store.put_create(keys::CLUSTER, body).await?.is_some() {
            info!(cluster_id = %meta.cluster_id, num_shards = meta.num_shards, "created cluster");
            return Ok(meta);
        }
        // Someone else created it first: read theirs.
    }
}

/// Check the bucket keeps the promises the protocol relies on: create-if-absent PUT,
/// If-Match PUT, strong read-after-write and strong LIST. A bucket that breaks any of them
/// would let two nodes believe they own the same shard, so clustered mode refuses it.
pub async fn self_test(store: &Store, node_id: &str) -> Result<(), WalError> {
    let key = format!("selftest/{node_id}/{}", uuid::Uuid::now_v7());
    let fail = |what: &str| {
        WalError::Config(format!(
            "storage self-test failed: {what}. This bucket cannot run a Valka cluster \
             (see docs: supported object stores)"
        ))
    };
    let result = async {
        let etag = store
            .put_create(&key, Bytes::from_static(b"1"))
            .await?
            .ok_or_else(|| fail("a fresh key already existed"))?;
        if store
            .put_create(&key, Bytes::from_static(b"2"))
            .await?
            .is_some()
        {
            return Err(fail("create-if-absent overwrote an existing object"));
        }
        let Some((bytes, read_etag)) = store.get(&key).await? else {
            return Err(fail("a written object was not readable right away"));
        };
        if &bytes[..] != b"1" {
            return Err(fail("read-after-write returned stale content"));
        }
        let etag = etag
            .or(read_etag)
            .ok_or_else(|| fail("the store returns no ETags"))?;
        let wrong = format!("\"{}\"", uuid::Uuid::now_v7());
        if matches!(
            store
                .put_if_match(&key, &wrong, Bytes::from_static(b"3"))
                .await?,
            CasOutcome::Written(_)
        ) {
            return Err(fail("If-Match accepted a wrong ETag"));
        }
        if !matches!(
            store
                .put_if_match(&key, &etag, Bytes::from_static(b"4"))
                .await?,
            CasOutcome::Written(_)
        ) {
            return Err(fail("If-Match rejected the current ETag"));
        }
        let listed = store.list(&format!("selftest/{node_id}/")).await?;
        if !listed.iter().any(|(k, _)| *k == key) {
            return Err(fail("LIST did not show an object written just before"));
        }
        Ok(())
    }
    .await;
    let _ = store.delete(&key).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn created_once_then_validated() {
        let s = Store::memory();
        let a = ensure(&s, 0).await.unwrap();
        assert_eq!(a.num_shards, DEFAULT_NUM_SHARDS);
        let b = ensure(&s, 4096).await.unwrap();
        assert_eq!(a, b);
        assert!(
            ensure(&s, 256)
                .await
                .unwrap_err()
                .to_string()
                .contains("never changes")
        );
        assert!(ensure(&Store::memory(), 1000).await.is_err());
        let small = ensure(&Store::memory(), 256).await.unwrap();
        assert_eq!(small.num_shards, 256);
    }

    #[tokio::test]
    async fn memory_and_local_pass_the_self_test() {
        self_test(&Store::memory(), "n").await.unwrap();
        let dir = std::env::temp_dir().join(format!("valka-selftest-{}", uuid::Uuid::now_v7()));
        let s = Store::local(dir.to_str().unwrap()).unwrap();
        self_test(&s, "n").await.unwrap();
        assert!(
            s.list("selftest/").await.unwrap().is_empty(),
            "probe cleaned up"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
