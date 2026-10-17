//! Per-shard snapshots: zstd-compressed JSON of whatever the engine hands us.
//!
//! Key: `snapshots/{hh}/{shard}/{gen}-{shard_seq}-{ptr}.snap`. Snapshots of a shard are
//! ordered by `(gen, shard_seq)`; the newest is the shard's state. The log pointer `ptr`
//! says where records after the snapshot may be found, and lives in the key so garbage
//! collection can run on a LIST alone.

use bytes::Bytes;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use std::fmt;
use valka_core::ShardId;

use crate::error::WalError;
use crate::lsn::keys;
use crate::store::Store;

const ZSTD_LEVEL: i32 = 3;

/// Where records after a snapshot live.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LogPtr {
    /// In `node`'s log, incarnation `inc`, at segment positions `>= seq`.
    At { node: String, inc: u32, seq: u64 },
    /// Nothing follows at this generation: the owner released the shard.
    Final,
    /// Nothing follows at this generation: written by splitting a dead incarnation's log.
    Split,
}

impl LogPtr {
    fn rank(&self) -> u8 {
        match self {
            LogPtr::At { .. } => 0,
            LogPtr::Split => 1,
            LogPtr::Final => 2,
        }
    }

    pub fn parse(s: &str) -> Option<LogPtr> {
        match s {
            "final" => Some(LogPtr::Final),
            "split" => Some(LogPtr::Split),
            _ => {
                let mut it = s.rsplitn(3, '.');
                let seq = it.next()?.parse().ok()?;
                let inc = it.next()?.parse().ok()?;
                let node = it.next()?;
                if node.is_empty() {
                    return None;
                }
                Some(LogPtr::At {
                    node: node.to_string(),
                    inc,
                    seq,
                })
            }
        }
    }
}

impl fmt::Display for LogPtr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogPtr::At { node, inc, seq } => write!(f, "{node}.{inc}.{seq}"),
            LogPtr::Final => write!(f, "final"),
            LogPtr::Split => write!(f, "split"),
        }
    }
}

/// Identity of one snapshot object.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnapshotKey {
    pub shard: ShardId,
    pub generation: u64,
    /// Last `shard_seq` reflected in the snapshot.
    pub seq: u64,
    pub ptr: LogPtr,
}

impl Ord for SnapshotKey {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.shard, self.generation, self.seq, self.ptr.rank())
            .cmp(&(o.shard, o.generation, o.seq, o.ptr.rank()))
            .then_with(|| self.ptr.to_string().cmp(&o.ptr.to_string()))
    }
}

impl PartialOrd for SnapshotKey {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl SnapshotKey {
    pub fn key(&self) -> String {
        format!(
            "{}{:012}-{:020}-{}.snap",
            keys::snapshot_prefix(self.shard),
            self.generation,
            self.seq,
            self.ptr
        )
    }

    pub fn parse(key: &str) -> Option<SnapshotKey> {
        let rest = key.strip_prefix(keys::SNAPSHOTS_PREFIX)?;
        let mut parts = rest.splitn(3, '/');
        let _hh = parts.next()?;
        let shard: u16 = parts.next()?.parse().ok()?;
        let file = parts.next()?.strip_suffix(".snap")?;
        let (generation, rest) = file.split_once('-')?;
        let (seq, ptr) = rest.split_once('-')?;
        Some(SnapshotKey {
            shard: ShardId(shard),
            generation: generation.parse().ok()?,
            seq: seq.parse().ok()?,
            ptr: LogPtr::parse(ptr)?,
        })
    }
}

fn encode<T: Serialize>(state: &T) -> Result<Bytes, WalError> {
    let json = serde_json::to_vec(state)?;
    Ok(Bytes::from(zstd::encode_all(&json[..], ZSTD_LEVEL)?))
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, WalError> {
    let json = zstd::decode_all(bytes)?;
    Ok(serde_json::from_slice(&json)?)
}

/// Write a snapshot, create-if-absent. `Ok(false)` if that exact key already existed:
/// two writers of one key (concurrent splitters) produce the same state, so the first wins.
pub async fn write<T: Serialize>(
    store: &Store,
    key: &SnapshotKey,
    state: &T,
) -> Result<bool, WalError> {
    Ok(store
        .put_create(&key.key(), encode(state)?)
        .await?
        .is_some())
}

/// Every snapshot of a shard, oldest first.
pub async fn list(store: &Store, shard: ShardId) -> Result<Vec<SnapshotKey>, WalError> {
    let mut out: Vec<SnapshotKey> = store
        .list(&keys::snapshot_prefix(shard))
        .await?
        .into_iter()
        .filter_map(|(k, _)| SnapshotKey::parse(&k))
        .filter(|k| k.shard == shard)
        .collect();
    out.sort();
    Ok(out)
}

/// Every snapshot in the bucket, grouped by shard, oldest first. One LIST.
pub async fn list_all(store: &Store) -> Result<BTreeMap<ShardId, Vec<SnapshotKey>>, WalError> {
    let mut out: BTreeMap<ShardId, Vec<SnapshotKey>> = BTreeMap::new();
    for (k, _) in store.list(keys::SNAPSHOTS_PREFIX).await? {
        if let Some(sk) = SnapshotKey::parse(&k) {
            out.entry(sk.shard).or_default().push(sk);
        }
    }
    for v in out.values_mut() {
        v.sort();
    }
    Ok(out)
}

pub async fn load<T: DeserializeOwned>(store: &Store, key: &SnapshotKey) -> Result<T, WalError> {
    let (bytes, _) = store
        .get(&key.key())
        .await?
        .ok_or_else(|| WalError::Corrupt(format!("snapshot {} vanished", key.key())))?;
    decode(&bytes)
}

/// Load the newest loadable snapshot among `candidates` (sorted oldest first) that passes
/// `accept`. A corrupt newest snapshot falls back to the previous one.
pub async fn load_newest_of<T: DeserializeOwned>(
    store: &Store,
    candidates: &[SnapshotKey],
    accept: impl Fn(&SnapshotKey) -> bool,
) -> Result<Option<(SnapshotKey, T)>, WalError> {
    for key in candidates.iter().rev().filter(|k| accept(k)) {
        let Some((bytes, _)) = store.get(&key.key()).await? else {
            continue;
        };
        match decode::<T>(&bytes) {
            Ok(state) => return Ok(Some((key.clone(), state))),
            Err(e) => {
                tracing::error!(key = %key.key(), error = %e, "corrupt snapshot, trying previous")
            }
        }
    }
    Ok(None)
}

/// Load the newest snapshot of a shard. `Ok(None)` when the shard has never been served.
pub async fn load_latest<T: DeserializeOwned>(
    store: &Store,
    shard: ShardId,
) -> Result<Option<(SnapshotKey, T)>, WalError> {
    let all = list(store, shard).await?;
    load_newest_of(store, &all, |_| true).await
}

/// Delete all but the newest `keep` snapshots of a shard (never the newest).
pub async fn prune(store: &Store, shard: ShardId, keep: usize) -> Result<usize, WalError> {
    let snaps = list(store, shard).await?;
    prune_listed(store, &snaps, keep).await
}

/// Like [`prune`] over an already-listed, sorted set.
pub async fn prune_listed(
    store: &Store,
    snaps: &[SnapshotKey],
    keep: usize,
) -> Result<usize, WalError> {
    let keep = keep.max(1);
    if snaps.len() <= keep {
        return Ok(0);
    }
    let n = snaps.len() - keep;
    for k in &snaps[..n] {
        store.delete(&k.key()).await?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(node: &str, inc: u32, seq: u64) -> LogPtr {
        LogPtr::At {
            node: node.into(),
            inc,
            seq,
        }
    }

    fn sk(shard: u16, generation: u64, seq: u64, ptr: LogPtr) -> SnapshotKey {
        SnapshotKey {
            shard: ShardId(shard),
            generation,
            seq,
            ptr,
        }
    }

    #[test]
    fn keys_round_trip_and_sort_by_gen_then_seq() {
        for ptr in [
            at("0192f0c4-7d1e-7c3a-9d52-1f2e3d4c5b6a", 3, 17),
            LogPtr::Final,
            LogPtr::Split,
        ] {
            let k = sk(9, 4, 120, ptr);
            assert_eq!(SnapshotKey::parse(&k.key()), Some(k.clone()));
        }
        let mut v = [
            sk(1, 2, 5, LogPtr::Final),
            sk(1, 1, 900, at("a", 1, 1)),
            sk(1, 2, 5, at("b", 1, 9)),
            sk(1, 2, 3, at("b", 1, 2)),
        ];
        v.sort();
        assert_eq!(v[0].generation, 1);
        assert_eq!((v[1].generation, v[1].seq), (2, 3));
        assert_eq!(v[3].ptr, LogPtr::Final, "a final snapshot wins a tie");
        let mut keys: Vec<String> = v.iter().map(|k| k.key()).collect();
        let sorted = keys.clone();
        keys.sort();
        assert_eq!(keys[0], sorted[0], "key strings sort by generation first");
    }

    #[tokio::test]
    async fn latest_wins_create_only_and_prune_keeps_newest() {
        let s = Store::memory();
        let a = sk(9, 1, 5, at("n", 1, 1));
        let b = sk(9, 1, 9, at("n", 1, 4));
        let c = sk(9, 2, 9, at("m", 1, 1));
        assert!(write(&s, &a, &vec![1, 2]).await.unwrap());
        assert!(write(&s, &b, &vec![3]).await.unwrap());
        assert!(write(&s, &c, &vec![4, 5, 6]).await.unwrap());
        assert!(!write(&s, &c, &vec![0]).await.unwrap(), "create-only");
        let (k, v): (SnapshotKey, Vec<i32>) = load_latest(&s, ShardId(9)).await.unwrap().unwrap();
        assert_eq!(k, c);
        assert_eq!(v, vec![4, 5, 6]);
        let all = list_all(&s).await.unwrap();
        assert_eq!(all[&ShardId(9)].len(), 3);
        assert_eq!(prune(&s, ShardId(9), 1).await.unwrap(), 2);
        assert_eq!(list(&s, ShardId(9)).await.unwrap(), vec![c]);
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
        let good = sk(1, 1, 1, LogPtr::Final);
        write(&s, &good, &"good".to_string()).await.unwrap();
        let bad = sk(1, 1, 2, LogPtr::Final);
        s.put(&bad.key(), Bytes::from_static(b"garbage"))
            .await
            .unwrap();
        let (k, v): (SnapshotKey, String) = load_latest(&s, ShardId(1)).await.unwrap().unwrap();
        assert_eq!(k, good);
        assert_eq!(v, "good");
    }
}
