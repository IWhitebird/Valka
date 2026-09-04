//! Reading segments back, in LSN order.

use crate::error::WalError;
use crate::lsn::{Lsn, keys};
use crate::record::Envelope;
use crate::segment;
use crate::store::Store;
use tracing::warn;

/// List a node's segments with LSN strictly greater than `after`, sorted ascending.
pub async fn list_segments(
    store: &Store,
    node_id: &str,
    after: Option<Lsn>,
) -> Result<Vec<(Lsn, String)>, WalError> {
    let items = store.list(&keys::wal_prefix(node_id)).await?;
    let mut out = Vec::with_capacity(items.len());
    for (key, _) in items {
        match keys::segment_lsn(&key) {
            Some(lsn) => {
                if after.is_none_or(|a| lsn > a) {
                    out.push((lsn, key));
                }
            }
            None => warn!(key, "ignoring foreign object under wal prefix"),
        }
    }
    out.sort();
    Ok(out)
}

/// Fetch and decode one segment.
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

/// Read every record after `after` (exclusive), tagged with its segment LSN.
pub async fn read_all(
    store: &Store,
    node_id: &str,
    after: Option<Lsn>,
) -> Result<Vec<(Lsn, Envelope)>, WalError> {
    let mut out = Vec::new();
    for (lsn, key) in list_segments(store, node_id, after).await? {
        for e in read_segment(store, &key).await? {
            out.push((lsn, e));
        }
    }
    Ok(out)
}

/// Stream segments to a callback, in order. Stops at the first error.
pub async fn replay<F>(
    store: &Store,
    node_id: &str,
    after: Option<Lsn>,
    mut f: F,
) -> Result<Option<Lsn>, WalError>
where
    F: FnMut(Lsn, Envelope),
{
    let mut last = None;
    for (lsn, key) in list_segments(store, node_id, after).await? {
        for e in read_segment(store, &key).await? {
            f(lsn, e);
        }
        last = Some(lsn);
    }
    Ok(last)
}

/// Delete segments with LSN strictly below `below`. Returns the number removed.
pub async fn truncate_before(store: &Store, node_id: &str, below: Lsn) -> Result<usize, WalError> {
    let mut n = 0;
    for (lsn, key) in list_segments(store, node_id, None).await? {
        if lsn < below {
            store.delete(&key).await?;
            n += 1;
        }
    }
    Ok(n)
}
