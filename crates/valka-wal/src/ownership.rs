//! Shard ownership: the `assignment` object and the flush-time verification hook.
//!
//! Phase 1 (single node) claims every shard at startup and verifies with a conditional
//! GET on every commit. Phase 2 adds takeover; the data model here already supports it.

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use valka_core::{NUM_SHARDS, ShardId};

use crate::error::WalError;
use crate::lsn::keys;
use crate::store::{CasOutcome, Conditional, Store};

/// One shard's owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardOwner {
    pub node: String,
    pub epoch: u32,
}

/// The whole cluster's shard map. Small (≈ 4096 × ~40 B) and CAS'd as a unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub version: u64,
    pub shards: Vec<Option<ShardOwner>>,
}

impl Assignment {
    pub fn empty() -> Self {
        Self {
            version: 0,
            shards: vec![None; NUM_SHARDS as usize],
        }
    }

    pub fn owner(&self, shard: ShardId) -> Option<&ShardOwner> {
        self.shards.get(shard.0 as usize).and_then(|o| o.as_ref())
    }

    pub fn owned_by(&self, node: &str) -> Vec<ShardId> {
        self.shards
            .iter()
            .enumerate()
            .filter_map(|(i, o)| match o {
                Some(o) if o.node == node => Some(ShardId(i as u16)),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipVerdict {
    Unchanged,
    Lost(Vec<ShardId>),
}

/// Called by the WAL committer after every segment PUT, before acks are released.
#[async_trait]
pub trait OwnershipCheck: Send + Sync {
    async fn verify(&self, shards: &[ShardId]) -> Result<OwnershipVerdict, WalError>;
}

/// Trusts itself unconditionally. Only for tests and `backend = "memory"` demos.
pub struct SingleNodeOwnership;

#[async_trait]
impl OwnershipCheck for SingleNodeOwnership {
    async fn verify(&self, _shards: &[ShardId]) -> Result<OwnershipVerdict, WalError> {
        Ok(OwnershipVerdict::Unchanged)
    }
}

/// Bucket-backed ownership. Holds the cached assignment + ETag; `verify` is one
/// conditional GET that returns 304 in the steady state.
pub struct Ownership {
    store: Store,
    node_id: String,
    cached: Mutex<Cached>,
}

#[derive(Clone)]
struct Cached {
    assignment: Assignment,
    etag: Option<String>,
    /// Epoch this node writes with. All of its shards share it in phase 1.
    epoch: u32,
}

impl Ownership {
    /// Claim every shard for this node, bumping the epoch past any previous owner.
    /// Retries CAS conflicts until it wins.
    pub async fn claim_all(store: Store, node_id: &str) -> Result<Arc<Self>, WalError> {
        loop {
            let current = store.get(keys::ASSIGNMENT).await?;
            let (mut assignment, etag) = match &current {
                Some((bytes, etag)) => (
                    serde_json::from_slice::<Assignment>(bytes)
                        .map_err(|e| WalError::Corrupt(format!("assignment: {e}")))?,
                    etag.clone(),
                ),
                None => (Assignment::empty(), None),
            };
            if assignment.shards.len() != NUM_SHARDS as usize {
                return Err(WalError::Corrupt(format!(
                    "assignment has {} shards, expected {NUM_SHARDS}",
                    assignment.shards.len()
                )));
            }
            let max_epoch = assignment
                .shards
                .iter()
                .flatten()
                .map(|o| o.epoch)
                .max()
                .unwrap_or(0);
            let epoch = max_epoch + 1;
            for s in assignment.shards.iter_mut() {
                *s = Some(ShardOwner {
                    node: node_id.to_string(),
                    epoch,
                });
            }
            assignment.version += 1;
            let body = Bytes::from(serde_json::to_vec(&assignment)?);

            let new_etag = match etag {
                None => match store.put_create(keys::ASSIGNMENT, body).await? {
                    Some(etag) => etag,
                    None => continue, // someone created it first; reload
                },
                Some(etag) => match store.put_if_match(keys::ASSIGNMENT, &etag, body).await? {
                    CasOutcome::Written(new) => new,
                    CasOutcome::Conflict => continue,
                },
            };
            // Some backends do not return an ETag on PUT; fetch it.
            let etag = match new_etag {
                Some(e) => Some(e),
                None => store.head_etag(keys::ASSIGNMENT).await?.flatten(),
            };
            tracing::info!(node = node_id, epoch, "claimed all shards");
            return Ok(Arc::new(Self {
                store,
                node_id: node_id.to_string(),
                cached: Mutex::new(Cached {
                    assignment,
                    etag,
                    epoch,
                }),
            }));
        }
    }

    pub fn epoch(&self) -> u32 {
        self.cached.lock().epoch
    }

    pub fn assignment(&self) -> Assignment {
        self.cached.lock().assignment.clone()
    }

    pub fn owns(&self, shard: ShardId) -> bool {
        let c = self.cached.lock();
        c.assignment
            .owner(shard)
            .is_some_and(|o| o.node == self.node_id && o.epoch == c.epoch)
    }
}

#[async_trait]
impl OwnershipCheck for Ownership {
    async fn verify(&self, shards: &[ShardId]) -> Result<OwnershipVerdict, WalError> {
        let (etag, epoch) = {
            let c = self.cached.lock();
            (c.etag.clone(), c.epoch)
        };
        let fresh = match etag {
            Some(etag) => match self
                .store
                .get_if_none_match(keys::ASSIGNMENT, &etag)
                .await?
            {
                Conditional::NotModified => return Ok(OwnershipVerdict::Unchanged),
                Conditional::Modified { bytes, etag } => Some((bytes, etag)),
                Conditional::Missing => None,
            },
            None => self.store.get(keys::ASSIGNMENT).await?,
        };
        let Some((bytes, etag)) = fresh else {
            // Assignment vanished: treat as losing everything. Someone wiped the bucket.
            return Ok(OwnershipVerdict::Lost(shards.to_vec()));
        };
        let assignment: Assignment = serde_json::from_slice(&bytes)
            .map_err(|e| WalError::Corrupt(format!("assignment: {e}")))?;
        let lost: Vec<ShardId> = shards
            .iter()
            .copied()
            .filter(|s| {
                !assignment
                    .owner(*s)
                    .is_some_and(|o| o.node == self.node_id && o.epoch == epoch)
            })
            .collect();
        {
            let mut c = self.cached.lock();
            c.assignment = assignment;
            c.etag = etag;
        }
        if lost.is_empty() {
            Ok(OwnershipVerdict::Unchanged)
        } else {
            Ok(OwnershipVerdict::Lost(lost))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn claim_bumps_epoch_and_verifies() {
        let s = Store::memory();
        let a = Ownership::claim_all(s.clone(), "a").await.unwrap();
        assert_eq!(a.epoch(), 1);
        assert!(a.owns(ShardId(0)));
        assert_eq!(
            a.verify(&[ShardId(0), ShardId(4095)]).await.unwrap(),
            OwnershipVerdict::Unchanged
        );

        // A second node takes over everything.
        let b = Ownership::claim_all(s.clone(), "b").await.unwrap();
        assert_eq!(b.epoch(), 2);
        assert_eq!(
            a.verify(&[ShardId(3), ShardId(4)]).await.unwrap(),
            OwnershipVerdict::Lost(vec![ShardId(3), ShardId(4)])
        );
        assert!(!a.owns(ShardId(3)));
        assert_eq!(
            b.verify(&[ShardId(3)]).await.unwrap(),
            OwnershipVerdict::Unchanged
        );
    }

    #[tokio::test]
    async fn concurrent_claims_serialise() {
        let s = Store::memory();
        let mut hs = Vec::new();
        for i in 0..8 {
            let s = s.clone();
            hs.push(tokio::spawn(async move {
                Ownership::claim_all(s, &format!("n{i}"))
                    .await
                    .unwrap()
                    .epoch()
            }));
        }
        let mut epochs: Vec<u32> = futures::future::join_all(hs)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        epochs.sort();
        assert_eq!(
            epochs,
            (1..=8).collect::<Vec<_>>(),
            "every claim gets a distinct epoch"
        );
    }
}
