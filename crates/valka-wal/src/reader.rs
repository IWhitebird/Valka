//! Reading segments back, and own-log truncation.

use crate::error::WalError;
use crate::lsn::{Lsn, keys};
use crate::record::Envelope;
use crate::segment;
use crate::store::Store;

/// One incarnation's segments, sorted ascending.
pub async fn list_segments(
    store: &Store,
    node_id: &str,
    inc: u32,
) -> Result<Vec<(Lsn, String)>, WalError> {
    let mut out: Vec<(Lsn, String)> = store
        .list(&keys::wal_prefix(node_id))
        .await?
        .into_iter()
        .filter_map(|(key, _)| {
            let lsn = keys::parse_segment(&key)
                .filter(|(n, l)| *n == node_id && l.inc == inc)
                .map(|(_, l)| l)?;
            Some((lsn, key))
        })
        .collect();
    out.sort();
    Ok(out)
}

/// Every segment of a node (all incarnations), sorted ascending.
pub async fn list_node_segments(
    store: &Store,
    node_id: &str,
) -> Result<Vec<(Lsn, String)>, WalError> {
    let mut out: Vec<(Lsn, String)> = store
        .list(&keys::wal_prefix(node_id))
        .await?
        .into_iter()
        .filter_map(|(key, _)| {
            let lsn = keys::parse_segment(&key)
                .filter(|(n, _)| *n == node_id)
                .map(|(_, l)| l)?;
            Some((lsn, key))
        })
        .collect();
    out.sort();
    Ok(out)
}

/// Fetch and decode one segment. Sealed segments decode to no records.
pub async fn read_segment(store: &Store, key: &str) -> Result<Vec<Envelope>, WalError> {
    let (bytes, _) = store
        .get(key)
        .await?
        .ok_or_else(|| WalError::Corrupt(format!("segment {key} vanished during replay")))?;
    let (header, records) = segment::decode(&bytes)?;
    if Some(header.lsn) != keys::segment_lsn(key) {
        return Err(WalError::Corrupt(format!(
            "segment {key} header lsn {} does not match key",
            header.lsn
        )));
    }
    Ok(records)
}

/// Every record of one incarnation, in order (diagnostics and tests).
pub async fn read_all(
    store: &Store,
    node_id: &str,
    inc: u32,
) -> Result<Vec<(Lsn, Envelope)>, WalError> {
    let mut out = Vec::new();
    for (lsn, key) in list_segments(store, node_id, inc).await? {
        for e in read_segment(store, &key).await? {
            out.push((lsn, e));
        }
    }
    Ok(out)
}

/// Every record a node ever wrote that is still in the bucket, all incarnations, in order.
pub async fn read_node(store: &Store, node_id: &str) -> Result<Vec<(Lsn, Envelope)>, WalError> {
    let mut out = Vec::new();
    for (lsn, key) in list_node_segments(store, node_id).await? {
        for e in read_segment(store, &key).await? {
            out.push((lsn, e));
        }
    }
    Ok(out)
}

/// Delete one incarnation's segments strictly below `below`, ascending and one at a time,
/// stopping at the first failure, so what remains is always a contiguous suffix. The
/// caller must have raised the lease's truncation watermark to `below` first.
pub async fn truncate_before(
    store: &Store,
    node_id: &str,
    inc: u32,
    below: u64,
) -> Result<usize, WalError> {
    let mut n = 0;
    for (lsn, key) in list_segments(store, node_id, inc).await? {
        if lsn.seq >= below {
            break;
        }
        store.delete(&key).await?;
        n += 1;
    }
    Ok(n)
}
