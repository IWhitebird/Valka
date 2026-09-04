//! Per-shard snapshots: zstd-compressed JSON of whatever the engine hands us.

use bytes::Bytes;
use serde::{Serialize, de::DeserializeOwned};
use valka_core::ShardId;

use crate::error::WalError;
use crate::lsn::{Lsn, keys};
use crate::store::Store;

const ZSTD_LEVEL: i32 = 3;

/// Write `state` as the snapshot of `shard` at `lsn`. The LSN is the writer's *next*
/// LSN at the moment the shard was serialised, i.e. every record in segments `< lsn` is
/// reflected and none from `>= lsn` are.
pub async fn write<T: Serialize>(
    store: &Store,
    shard: ShardId,
    lsn: Lsn,
    state: &T,
) -> Result<(), WalError> {
    let json = serde_json::to_vec(state)?;
    let body = zstd::encode_all(&json[..], ZSTD_LEVEL)?;
    store
        .put(&keys::snapshot(shard, lsn), Bytes::from(body))
        .await?;
    Ok(())
}

/// Sorted list of `(lsn, key)` snapshots for a shard.
pub async fn list(store: &Store, shard: ShardId) -> Result<Vec<(Lsn, String)>, WalError> {
    let items = store.list(&keys::snapshot_prefix(shard)).await?;
    let mut out: Vec<(Lsn, String)> = items
        .into_iter()
        .filter_map(|(k, _)| keys::snapshot_lsn(&k).map(|l| (l, k)))
        .collect();
    out.sort();
    Ok(out)
}

/// Load the newest snapshot for a shard. `Ok(None)` when the shard has never been
/// snapshotted. A corrupt newest snapshot falls back to the previous one.
pub async fn load_latest<T: DeserializeOwned>(
    store: &Store,
    shard: ShardId,
) -> Result<Option<(Lsn, T)>, WalError> {
    let mut snaps = list(store, shard).await?;
    while let Some((lsn, key)) = snaps.pop() {
        let Some((bytes, _)) = store.get(&key).await? else {
            continue;
        };
        match decode::<T>(&bytes) {
            Ok(state) => return Ok(Some((lsn, state))),
            Err(e) => {
                tracing::error!(key, error = %e, "corrupt snapshot, trying previous");
            }
        }
    }
    Ok(None)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, WalError> {
    let json = zstd::decode_all(bytes)?;
    Ok(serde_json::from_slice(&json)?)
}

/// Delete all but the newest `keep` snapshots of a shard.
pub async fn prune(store: &Store, shard: ShardId, keep: usize) -> Result<usize, WalError> {
    let snaps = list(store, shard).await?;
    if snaps.len() <= keep {
        return Ok(0);
    }
    let n = snaps.len() - keep;
    for (_, key) in snaps.into_iter().take(n) {
        store.delete(&key).await?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn latest_wins_and_prune_keeps_newest() {
        let s = Store::memory();
        let sh = ShardId(9);
        write(&s, sh, Lsn::new(1, 5), &vec![1, 2]).await.unwrap();
        write(&s, sh, Lsn::new(1, 9), &vec![3]).await.unwrap();
        write(&s, sh, Lsn::new(2, 1), &vec![4, 5, 6]).await.unwrap();
        let (lsn, v): (Lsn, Vec<i32>) = load_latest(&s, sh).await.unwrap().unwrap();
        assert_eq!(lsn, Lsn::new(2, 1));
        assert_eq!(v, vec![4, 5, 6]);
        assert_eq!(prune(&s, sh, 1).await.unwrap(), 2);
        assert_eq!(list(&s, sh).await.unwrap().len(), 1);
        assert!(
            load_latest::<Vec<i32>>(&s, ShardId(10))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn corrupt_latest_falls_back() {
        let s = Store::memory();
        let sh = ShardId(1);
        write(&s, sh, Lsn::new(1, 1), &"good".to_string())
            .await
            .unwrap();
        s.put(
            &keys::snapshot(sh, Lsn::new(1, 2)),
            Bytes::from_static(b"garbage"),
        )
        .await
        .unwrap();
        let (lsn, v): (Lsn, String) = load_latest(&s, sh).await.unwrap().unwrap();
        assert_eq!(lsn, Lsn::new(1, 1));
        assert_eq!(v, "good");
    }
}
