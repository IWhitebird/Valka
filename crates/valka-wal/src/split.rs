//! Reading a dead incarnation's log to its sealed end, and the split markers.
//!
//! A dead incarnation `D = (node, inc)` is read from its truncation watermark up to the
//! first gap. The gap is sealed with an empty SEALED segment (create-if-absent), so a late
//! PUT from D can never fill it and every reader agrees where the log ends. Segments above
//! the seal, if any landed out of order, were never acknowledged: acks resolve strictly in
//! segment order and the gap below them never did.
//!
//! Positions at or above the watermark are never deleted before D's split marker exists
//! (truncation raises the watermark first), so a missing position there is the end of the
//! log, never a truncation hole.

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::error::WalError;
use crate::lsn::{Lsn, keys};
use crate::record::Envelope;
use crate::segment;
use crate::store::Store;

/// Segments fetched concurrently while reading a log.
const READ_CONCURRENCY: usize = 16;

/// A dead incarnation's log, read to its seal.
#[derive(Debug, Default)]
pub struct SealedLog {
    /// Every record from the watermark to the seal, in log order, with its segment.
    pub records: Vec<(Lsn, Envelope)>,
    /// Position of the SEALED segment: the log ends just before it.
    pub end: u64,
    pub segments_read: usize,
}

/// Read `node`'s incarnation `inc` from `trunc` to its end, sealing the end if needed.
pub async fn read_sealed(
    store: &Store,
    node: &str,
    inc: u32,
    trunc: u64,
) -> Result<SealedLog, WalError> {
    let trunc = trunc.max(1);
    let listed: BTreeSet<u64> = store
        .list(&keys::wal_prefix(node))
        .await?
        .into_iter()
        .filter_map(|(k, _)| {
            keys::parse_segment(&k)
                .filter(|(n, l)| *n == node && l.inc == inc && l.seq >= trunc)
                .map(|(_, l)| l.seq)
        })
        .collect();
    let mut run_end = trunc;
    while listed.contains(&run_end) {
        run_end += 1;
    }

    let mut out = SealedLog::default();
    // The contiguous run the LIST saw, fetched concurrently, consumed in order.
    let mut fetched = futures::stream::iter(trunc..run_end)
        .map(|seq| fetch(store, node, Lsn::new(inc, seq)))
        .buffered(READ_CONCURRENCY);
    while let Some(r) = fetched.next().await {
        let (lsn, seg) = r?;
        match seg {
            Fetched::Sealed => {
                out.end = lsn.seq;
                return Ok(out);
            }
            Fetched::Records(recs) => {
                out.segments_read += 1;
                out.records.extend(recs.into_iter().map(|e| (lsn, e)));
            }
            Fetched::Missing => {
                return Err(WalError::Corrupt(format!(
                    "segment {} of {node} vanished while splitting (already split?)",
                    lsn
                )));
            }
        }
    }

    // At the first gap: seal it, or, if a late PUT just filled it, keep reading.
    let mut pos = run_end;
    loop {
        let lsn = Lsn::new(inc, pos);
        let key = keys::segment(node, lsn);
        if store
            .put_create(&key, segment::encode_sealed(lsn, node))
            .await?
            .is_some()
        {
            tracing::info!(node, inc, end = pos, "sealed log");
            out.end = pos;
            return Ok(out);
        }
        match fetch(store, node, lsn).await?.1 {
            Fetched::Sealed => {
                out.end = pos;
                return Ok(out);
            }
            Fetched::Records(recs) => {
                out.segments_read += 1;
                out.records.extend(recs.into_iter().map(|e| (lsn, e)));
                pos += 1;
            }
            Fetched::Missing => {
                return Err(WalError::Corrupt(format!(
                    "segment {lsn} of {node} exists and does not"
                )));
            }
        }
    }
}

enum Fetched {
    Records(Vec<Envelope>),
    Sealed,
    Missing,
}

async fn fetch(store: &Store, node: &str, lsn: Lsn) -> Result<(Lsn, Fetched), WalError> {
    let key = keys::segment(node, lsn);
    let Some((bytes, _)) = store.get(&key).await? else {
        return Ok((lsn, Fetched::Missing));
    };
    let (header, records) = segment::decode(&bytes)?;
    if header.lsn != lsn || header.node_id != node {
        return Err(WalError::Corrupt(format!(
            "segment {key} header says {}:{}",
            header.node_id, header.lsn
        )));
    }
    Ok((
        lsn,
        if header.sealed() {
            Fetched::Sealed
        } else {
            Fetched::Records(records)
        },
    ))
}

/// "This incarnation's log is fully split": every record in it is covered by snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitMarker {
    pub node: String,
    pub inc: u32,
    pub split_by: String,
    pub at: DateTime<Utc>,
    /// Position of the seal.
    pub end: u64,
    pub records: usize,
    pub snapshots_written: usize,
}

pub async fn marker_exists(store: &Store, node: &str, inc: u32) -> Result<bool, WalError> {
    Ok(store
        .head_etag(&keys::split_marker(node, inc))
        .await?
        .is_some())
}

/// Write the marker (create-if-absent; a concurrent splitter's identical marker wins).
pub async fn write_marker(store: &Store, marker: &SplitMarker) -> Result<(), WalError> {
    let body = Bytes::from(serde_json::to_vec(marker)?);
    store
        .put_create(&keys::split_marker(&marker.node, marker.inc), body)
        .await?;
    Ok(())
}

/// Delete every segment of one incarnation (after its split marker exists).
pub async fn delete_incarnation_log(
    store: &Store,
    node: &str,
    inc: u32,
) -> Result<usize, WalError> {
    let mut n = 0;
    for (lsn, key) in crate::reader::list_segments(store, node, inc).await? {
        debug_assert_eq!(lsn.inc, inc);
        store.delete(&key).await?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::WalRecord;
    use valka_core::ShardId;

    fn rec(i: u64) -> Envelope {
        Envelope::new(
            ShardId(1),
            WalRecord::TaskPromoted {
                task_id: format!("t{i}"),
            },
        )
    }

    async fn put_seg(s: &Store, node: &str, inc: u32, seq: u64) {
        let lsn = Lsn::new(inc, seq);
        s.put(
            &keys::segment(node, lsn),
            segment::encode(lsn, node, &[rec(seq)], true).unwrap(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn reads_to_the_first_gap_and_seals_it() {
        let s = Store::memory();
        for seq in [1, 2, 3, 5] {
            put_seg(&s, "n", 1, seq).await;
        }
        put_seg(&s, "n", 2, 1).await; // another incarnation: ignored
        let log = read_sealed(&s, "n", 1, 1).await.unwrap();
        assert_eq!(log.end, 4);
        assert_eq!(log.records.len(), 3);
        // The late segment 4 can no longer land.
        let late = segment::encode(Lsn::new(1, 4), "n", &[rec(4)], true).unwrap();
        assert!(
            s.put_create(&keys::segment("n", Lsn::new(1, 4)), late)
                .await
                .unwrap()
                .is_none()
        );
        // A second split agrees on the end and the records.
        let again = read_sealed(&s, "n", 1, 1).await.unwrap();
        assert_eq!(again.end, 4);
        assert_eq!(again.records.len(), 3);
    }

    #[tokio::test]
    async fn holes_below_the_watermark_are_not_the_end() {
        let s = Store::memory();
        // 1..=3 truncated (watermark 4), but 2 survived a failed delete.
        for seq in [2, 4, 5, 6] {
            put_seg(&s, "n", 1, seq).await;
        }
        let log = read_sealed(&s, "n", 1, 4).await.unwrap();
        assert_eq!(log.end, 7);
        assert_eq!(
            log.records.iter().map(|(l, _)| l.seq).collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
    }

    #[tokio::test]
    async fn fully_truncated_log_seals_at_the_watermark() {
        let s = Store::memory();
        let log = read_sealed(&s, "n", 3, 9).await.unwrap();
        assert_eq!(log.end, 9);
        assert!(log.records.is_empty());
        assert!(!marker_exists(&s, "n", 3).await.unwrap());
        write_marker(
            &s,
            &SplitMarker {
                node: "n".into(),
                inc: 3,
                split_by: "m".into(),
                at: Utc::now(),
                end: 9,
                records: 0,
                snapshots_written: 0,
            },
        )
        .await
        .unwrap();
        assert!(marker_exists(&s, "n", 3).await.unwrap());
        assert_eq!(delete_incarnation_log(&s, "n", 3).await.unwrap(), 1);
    }
}
