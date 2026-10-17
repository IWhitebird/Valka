//! Log positions and the bucket key layout.

use serde::{Deserialize, Serialize};
use std::fmt;
use valka_core::{ShardId, hash_prefix};

/// Position in one node's log: `(inc, seq)`. `inc` is the incarnation (one run of the
/// node process, bumped by the lease CAS at every start); `seq` numbers the segments that
/// incarnation writes, from 1, without gaps.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct Lsn {
    pub inc: u32,
    pub seq: u64,
}

impl Lsn {
    pub const ZERO: Lsn = Lsn { inc: 0, seq: 0 };

    pub fn new(inc: u32, seq: u64) -> Self {
        Self { inc, seq }
    }

    pub fn next(self) -> Self {
        Lsn {
            inc: self.inc,
            seq: self.seq + 1,
        }
    }

    /// Fixed-width, lexicographically sortable file stem: `0000000001-0000000000000042`.
    pub fn stem(&self) -> String {
        format!("{:010}-{:016}", self.inc, self.seq)
    }

    pub fn parse_stem(stem: &str) -> Option<Lsn> {
        let (i, s) = stem.split_once('-')?;
        if i.len() != 10 || s.len() != 16 {
            return None;
        }
        Some(Lsn {
            inc: i.parse().ok()?,
            seq: s.parse().ok()?,
        })
    }
}

impl fmt::Display for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.inc, self.seq)
    }
}

/// Node ids appear in keys and in snapshot log pointers (`{node}.{inc}.{seq}`), so they
/// are restricted to `[A-Za-z0-9_-]{1,64}`.
pub fn valid_node_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Bucket key layout. Everything under one place so the layout is greppable.
///
/// LIST prefixes must end at a `/`: `object_store` lists by path segment, not by string
/// prefix.
///
/// ```text
/// cluster.json                                   created once
/// nodes/{node}/lease                             liveness + fencing, CAS'd
/// owners/{hh}/{shard}                            shard owner + generation, CAS'd on moves
/// queues/{hh}/{queue}                            queue partition count
/// wal/{node}/{inc}-{seq}.seg                     WAL segments, create-if-absent
/// snapshots/{hh}/{shard}/{gen}-{seq}-{ptr}.snap  shard snapshots, ordered by (gen, seq)
/// splits/{node}/{inc}                            "this dead incarnation's log is split"
/// logs/{hh}/{run}/{chunk}.log                    task log chunks (not part of the WAL)
/// blobs/{hh}/{id}                                large payloads
/// ```
/// `{hh}` is two hex digits of a hash of the rest of the key.
pub mod keys {
    use super::*;

    pub const CLUSTER: &str = "cluster.json";
    pub const NODES_PREFIX: &str = "nodes/";
    pub const OWNERS_PREFIX: &str = "owners/";
    pub const WAL_PREFIX: &str = "wal/";
    pub const SNAPSHOTS_PREFIX: &str = "snapshots/";
    pub const SPLITS_PREFIX: &str = "splits/";
    pub const LOGS_PREFIX: &str = "logs/";
    pub const QUEUES_PREFIX: &str = "queues/";
    pub const BLOBS_PREFIX: &str = "blobs/";

    pub fn node_lease(node_id: &str) -> String {
        format!("nodes/{node_id}/lease")
    }

    /// Node id from a `nodes/{node}/lease` key.
    pub fn lease_node(key: &str) -> Option<&str> {
        key.strip_prefix(NODES_PREFIX)?.strip_suffix("/lease")
    }

    pub fn owner(shard: ShardId) -> String {
        let s = shard.to_string();
        format!("owners/{}/{s}", hash_prefix(&s))
    }

    pub fn queue(queue: &str) -> String {
        format!("queues/{}/{queue}", hash_prefix(queue))
    }

    pub fn wal_prefix(node_id: &str) -> String {
        format!("wal/{node_id}/")
    }

    pub fn segment(node_id: &str, lsn: Lsn) -> String {
        format!("wal/{node_id}/{}.seg", lsn.stem())
    }

    /// `(node, lsn)` from a segment key. `None` for foreign objects.
    pub fn parse_segment(key: &str) -> Option<(&str, Lsn)> {
        let rest = key.strip_prefix(WAL_PREFIX)?;
        let (node, file) = rest.split_once('/')?;
        let stem = file.strip_suffix(".seg")?;
        Some((node, Lsn::parse_stem(stem)?))
    }

    /// The LSN of a segment key. `None` for foreign objects.
    pub fn segment_lsn(key: &str) -> Option<Lsn> {
        parse_segment(key).map(|(_, l)| l)
    }

    pub fn snapshot_prefix(shard: ShardId) -> String {
        let s = shard.to_string();
        format!("snapshots/{}/{s}/", hash_prefix(&s))
    }

    pub fn split_marker(node_id: &str, inc: u32) -> String {
        format!("splits/{node_id}/{inc:010}")
    }

    /// `(node, inc)` from a split marker key.
    pub fn parse_split_marker(key: &str) -> Option<(&str, u32)> {
        let rest = key.strip_prefix(SPLITS_PREFIX)?;
        let (node, inc) = rest.split_once('/')?;
        Some((node, inc.parse().ok()?))
    }

    pub fn logs_prefix(run_id: &str) -> String {
        format!("logs/{}/{run_id}/", hash_prefix(run_id))
    }

    pub fn log_chunk(run_id: &str, chunk_id: &str) -> String {
        format!("logs/{}/{run_id}/{chunk_id}.log", hash_prefix(run_id))
    }

    /// Audit record of one admin action.
    pub fn admin_audit(ts_ms: i64, id: &str) -> String {
        format!("admin/{ts_ms:016}-{id}.json")
    }

    pub fn blob(id: &str) -> String {
        format!("blobs/{}/{id}", hash_prefix(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_sort_like_lsns() {
        let a = Lsn::new(1, 9);
        let b = Lsn::new(1, 10);
        let c = Lsn::new(2, 0);
        assert!(a < b && b < c);
        assert!(a.stem() < b.stem() && b.stem() < c.stem());
        assert_eq!(Lsn::parse_stem(&c.stem()), Some(c));
        assert_eq!(Lsn::parse_stem("1-2"), None);
    }

    #[test]
    fn segment_key_round_trip() {
        let l = Lsn::new(3, 77);
        let k = keys::segment("node-a", l);
        assert_eq!(k, "wal/node-a/0000000003-0000000000000077.seg");
        assert_eq!(keys::parse_segment(&k), Some(("node-a", l)));
        assert_eq!(keys::segment_lsn("wal/node-a/garbage"), None);
    }

    #[test]
    fn other_keys() {
        assert_eq!(keys::lease_node(&keys::node_lease("n-1")), Some("n-1"));
        let m = keys::split_marker("n", 7);
        assert_eq!(keys::parse_split_marker(&m), Some(("n", 7)));
        let o = keys::owner(ShardId(12));
        assert!(o.starts_with("owners/") && o.ends_with("/00012"));
        assert!(keys::snapshot_prefix(ShardId(12)).ends_with("/00012/"));
    }

    #[test]
    fn node_ids() {
        assert!(valid_node_id("valka-1"));
        assert!(valid_node_id("0192f0c4-7d1e-7c3a-9d52-1f2e3d4c5b6a"));
        assert!(!valid_node_id(""));
        assert!(!valid_node_id("a.b"));
        assert!(!valid_node_id("a/b"));
        assert!(!valid_node_id(&"x".repeat(65)));
    }
}
