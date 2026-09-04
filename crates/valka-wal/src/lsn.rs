//! Log sequence numbers and bucket key layout.

use serde::{Deserialize, Serialize};
use std::fmt;
use valka_core::ShardId;

/// Position in a node's log: `(epoch, seq)`. Epoch bumps on every ownership change so a
/// zombie writer's late segments sort before, and are ignored after, the takeover.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct Lsn {
    pub epoch: u32,
    pub seq: u64,
}

impl Lsn {
    pub const ZERO: Lsn = Lsn { epoch: 0, seq: 0 };

    pub fn new(epoch: u32, seq: u64) -> Self {
        Self { epoch, seq }
    }

    pub fn next(self) -> Self {
        Lsn {
            epoch: self.epoch,
            seq: self.seq + 1,
        }
    }

    /// Fixed-width, lexicographically sortable file stem: `00000001-0000000000000042`.
    pub fn stem(&self) -> String {
        format!("{:08}-{:016}", self.epoch, self.seq)
    }

    pub fn parse_stem(stem: &str) -> Option<Lsn> {
        let (e, s) = stem.split_once('-')?;
        Some(Lsn {
            epoch: e.parse().ok()?,
            seq: s.parse().ok()?,
        })
    }
}

impl fmt::Display for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.epoch, self.seq)
    }
}

/// Bucket key layout. Everything under one place so the layout is greppable.
pub mod keys {
    use super::*;

    pub const ASSIGNMENT: &str = "assignment";

    pub fn wal_prefix(node_id: &str) -> String {
        format!("wal/{node_id}/")
    }

    pub fn segment(node_id: &str, lsn: Lsn) -> String {
        format!("wal/{node_id}/{}.seg", lsn.stem())
    }

    /// Parse the LSN out of a segment key. Returns `None` for foreign objects.
    pub fn segment_lsn(key: &str) -> Option<Lsn> {
        let file = key.rsplit('/').next()?;
        let stem = file.strip_suffix(".seg")?;
        Lsn::parse_stem(stem)
    }

    pub fn snapshot_prefix(shard: ShardId) -> String {
        format!("snapshots/{shard}/")
    }

    pub fn snapshot(shard: ShardId, lsn: Lsn) -> String {
        format!("snapshots/{shard}/{}.snap", lsn.stem())
    }

    pub fn snapshot_lsn(key: &str) -> Option<Lsn> {
        let file = key.rsplit('/').next()?;
        let stem = file.strip_suffix(".snap")?;
        Lsn::parse_stem(stem)
    }

    pub fn node_lease(node_id: &str) -> String {
        format!("nodes/{node_id}/lease")
    }

    pub fn logs_prefix(run_id: &str) -> String {
        format!("logs/{run_id}/")
    }

    pub fn log_chunk(run_id: &str, chunk_id: &str) -> String {
        format!("logs/{run_id}/{chunk_id}.log")
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
    }

    #[test]
    fn segment_key_round_trip() {
        let l = Lsn::new(3, 77);
        let k = keys::segment("node-a", l);
        assert_eq!(keys::segment_lsn(&k), Some(l));
        assert_eq!(keys::segment_lsn("wal/node-a/garbage"), None);
    }
}
