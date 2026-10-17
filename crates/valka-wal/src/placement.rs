//! `placement.json`: operator overrides of where shards live (admin "move shard"). A pinned
//! shard's desired owner is the pinned node while that node is eligible; otherwise
//! rendezvous hashing decides as usual. CAS'd as one small object; nodes re-read it with
//! a conditional GET on every membership refresh.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::WalError;
use crate::store::{CasOutcome, Conditional, Store};

pub const KEY: &str = "placement.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// shard -> node id
    #[serde(default)]
    pub pins: BTreeMap<u16, String>,
    #[serde(default)]
    pub version: u64,
}

/// The current placement and its ETag (`None` when the object does not exist yet).
pub async fn get(store: &Store) -> Result<(Placement, Option<String>), WalError> {
    match store.get(KEY).await? {
        Some((bytes, etag)) => Ok((
            serde_json::from_slice(&bytes).map_err(|e| WalError::Corrupt(format!("{KEY}: {e}")))?,
            etag,
        )),
        None => Ok((Placement::default(), None)),
    }
}

/// Re-read only if changed since `etag`. `None` when unchanged.
pub async fn refresh(
    store: &Store,
    etag: Option<&str>,
) -> Result<Option<(Placement, Option<String>)>, WalError> {
    let Some(etag) = etag else {
        return Ok(Some(get(store).await?));
    };
    match store.get_if_none_match(KEY, etag).await? {
        Conditional::NotModified => Ok(None),
        Conditional::Modified { bytes, etag } => Ok(Some((
            serde_json::from_slice(&bytes).map_err(|e| WalError::Corrupt(format!("{KEY}: {e}")))?,
            etag,
        ))),
        Conditional::Missing => Ok(Some((Placement::default(), None))),
    }
}

/// Apply `change` with a CAS loop. Returns the new placement.
pub async fn update(store: &Store, change: impl Fn(&mut Placement)) -> Result<Placement, WalError> {
    loop {
        let (mut p, etag) = get(store).await?;
        change(&mut p);
        p.version += 1;
        let body = Bytes::from(serde_json::to_vec_pretty(&p)?);
        let ok = match etag {
            None => store.put_create(KEY, body).await?.is_some(),
            Some(e) => matches!(
                store.put_if_match(KEY, &e, body).await?,
                CasOutcome::Written(_)
            ),
        };
        if ok {
            return Ok(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pins_round_trip_and_refresh_is_conditional() {
        let s = Store::memory();
        let (p, etag) = get(&s).await.unwrap();
        assert!(p.pins.is_empty() && etag.is_none());
        let p = update(&s, |p| {
            p.pins.insert(7, "b".into());
        })
        .await
        .unwrap();
        assert_eq!(p.version, 1);
        let (_, etag) = get(&s).await.unwrap();
        assert!(refresh(&s, etag.as_deref()).await.unwrap().is_none());
        update(&s, |p| {
            p.pins.remove(&7);
        })
        .await
        .unwrap();
        let (p, _) = refresh(&s, etag.as_deref()).await.unwrap().unwrap();
        assert!(p.pins.is_empty());
        assert_eq!(p.version, 2);
    }
}
