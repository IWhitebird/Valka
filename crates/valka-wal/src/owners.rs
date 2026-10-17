//! Shard owner records: `owners/{hh}/{shard}`, one object per shard, CAS'd only when the
//! shard moves (claim, release). A stable cluster never writes them.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use valka_core::ShardId;

use crate::error::WalError;
use crate::lsn::keys;
use crate::store::{CasOutcome, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerState {
    /// Served by `(node, inc)` at generation `gen`.
    Owned,
    /// Handed back by its last owner; its final snapshot holds the state.
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerRecord {
    pub node: String,
    pub inc: u32,
    /// Bumped on every claim. Records and snapshots carry it; it orders snapshots of a shard
    /// written by different owners.
    #[serde(rename = "gen")]
    pub generation: u64,
    pub state: OwnerState,
}

/// An owner record with the ETag it was read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Versioned {
    pub record: OwnerRecord,
    pub etag: String,
}

pub async fn get(store: &Store, shard: ShardId) -> Result<Option<Versioned>, WalError> {
    let key = keys::owner(shard);
    let Some((bytes, etag)) = store.get(&key).await? else {
        return Ok(None);
    };
    let record: OwnerRecord =
        serde_json::from_slice(&bytes).map_err(|e| WalError::Corrupt(format!("{key}: {e}")))?;
    let etag = match etag {
        Some(e) => e,
        None => store.head_etag(&key).await?.flatten().ok_or_else(|| {
            WalError::Config("storage backend returns no ETags; CAS is impossible".into())
        })?,
    };
    Ok(Some(Versioned { record, etag }))
}

/// Replace the record read as `current` (or create it when `current` is `None`).
/// `Ok(false)` when someone else wrote it first.
pub async fn cas(
    store: &Store,
    shard: ShardId,
    current: Option<&Versioned>,
    next: &OwnerRecord,
) -> Result<bool, WalError> {
    let key = keys::owner(shard);
    let body = Bytes::from(serde_json::to_vec(next)?);
    match current {
        None => Ok(store.put_create(&key, body).await?.is_some()),
        Some(v) => Ok(matches!(
            store.put_if_match(&key, &v.etag, body).await?,
            CasOutcome::Written(_)
        )),
    }
}

/// Every owner record in the bucket. One LIST plus one GET per shard; used by the garbage
/// collector and the admin API, never on a hot path.
pub async fn list_all(store: &Store) -> Result<Vec<(ShardId, Versioned)>, WalError> {
    let mut out = Vec::new();
    for (key, _) in store.list(keys::OWNERS_PREFIX).await? {
        let Some(shard) = key.rsplit('/').next().and_then(|s| s.parse::<u16>().ok()) else {
            continue;
        };
        if let Some(v) = get(store, ShardId(shard)).await? {
            out.push((ShardId(shard), v));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(node: &str, inc: u32, generation: u64, state: OwnerState) -> OwnerRecord {
        OwnerRecord {
            node: node.into(),
            inc,
            generation,
            state,
        }
    }

    #[tokio::test]
    async fn create_then_cas_and_conflicts() {
        let s = Store::memory();
        let sh = ShardId(3);
        assert!(get(&s, sh).await.unwrap().is_none());
        assert!(
            cas(&s, sh, None, &rec("a", 1, 1, OwnerState::Owned))
                .await
                .unwrap()
        );
        assert!(
            !cas(&s, sh, None, &rec("b", 1, 1, OwnerState::Owned))
                .await
                .unwrap(),
            "second create loses"
        );
        let v = get(&s, sh).await.unwrap().unwrap();
        assert_eq!(v.record.node, "a");
        assert!(
            cas(&s, sh, Some(&v), &rec("a", 1, 1, OwnerState::Released))
                .await
                .unwrap()
        );
        assert!(
            !cas(&s, sh, Some(&v), &rec("c", 1, 2, OwnerState::Owned))
                .await
                .unwrap(),
            "stale etag loses"
        );
        let all = list_all(&s).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].1.record.state, OwnerState::Released);
    }
}
