//! Node leases: `nodes/{node}/lease`.
//!
//! The lease is a node's liveness signal and its fencing token. Only its owner writes it,
//! except for exactly one kind of write by another node: the guardian's tombstone. Every
//! write is a CAS.
//!
//! - **Start** CASes `inc := old.inc + 1`, which fences the previous run of this node id.
//! - **Renewal** CAS-bumps `counter` every `interval`. A conflict that turns out to be
//!   someone else's write (a tombstone, a newer incarnation) means this process is fenced:
//!   it poisons and exits.
//! - **Self-fencing**: if no renewal has succeeded for `timeout - safety_margin` (measured
//!   from when the last successful renewal was *sent*), the node stops serving until the
//!   next one succeeds, so it goes quiet before anyone can take over.
//! - **Flush check** ([`FenceCheck`]): after every segment PUT and before releasing acks the
//!   writer checks the lease with a conditional GET. The ordering PUT → check → tombstone →
//!   split LIST is what makes acked records visible to the splitter; it uses no clock.
//! - **Truncation watermark**: `trunc` is the lowest position of this incarnation's log that
//!   may still exist. It is raised (CAS) *before* segments below it are deleted, so a
//!   splitter that reads the frozen (tombstoned) lease never mistakes a truncation hole for
//!   the end of the log.

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

use crate::error::WalError;
use crate::lsn::keys;
use crate::store::{CasOutcome, Conditional, Store};

/// Node-to-node protocol version. Bumped when records, RPCs or bucket objects change in a
/// way an older node would misread.
pub const PROTOCOL: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    Alive,
    /// Shutting down: hands its shards over, takes no new ones.
    Draining,
    /// Tombstoned by a guardian, or marked by the node itself on a clean exit.
    Dead,
}

/// An older incarnation of this node whose log has not been split yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsplitInc {
    pub inc: u32,
    /// That incarnation's truncation watermark.
    pub trunc: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub node: String,
    pub inc: u32,
    pub state: LeaseState,
    pub counter: u64,
    /// Lowest position of incarnation `inc`'s log that may still exist.
    #[serde(default = "one")]
    pub trunc: u64,
    #[serde(default)]
    pub unsplit: Vec<UnsplitInc>,
    #[serde(default)]
    pub grpc_addr: Option<String>,
    #[serde(default)]
    pub http_addr: Option<String>,
    #[serde(default)]
    pub internal_addr: Option<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub protocol: u32,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub dead_by: Option<String>,
}

fn one() -> u64 {
    1
}

impl LeaseRecord {
    /// Every incarnation of this node whose log may still need splitting, with its
    /// truncation watermark. Includes the current one.
    pub fn incarnations(&self) -> Vec<UnsplitInc> {
        let mut v = self.unsplit.clone();
        v.push(UnsplitInc {
            inc: self.inc,
            trunc: self.trunc,
        });
        v.sort_by_key(|u| u.inc);
        v.dedup_by_key(|u| u.inc);
        v
    }

    /// `(node, inc)` can no longer write: a newer incarnation exists, or it was declared
    /// dead.
    pub fn is_dead_incarnation(&self, inc: u32) -> bool {
        inc < self.inc || (inc == self.inc && self.state == LeaseState::Dead)
    }
}

/// A lease as read, with its ETag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedLease {
    pub record: LeaseRecord,
    pub etag: String,
}

fn parse(key: &str, bytes: &[u8]) -> Result<LeaseRecord, WalError> {
    serde_json::from_slice(bytes).map_err(|e| WalError::Corrupt(format!("{key}: {e}")))
}

async fn etag_or_head(store: &Store, key: &str, etag: Option<String>) -> Result<String, WalError> {
    match etag {
        Some(e) => Ok(e),
        None => store.head_etag(key).await?.flatten().ok_or_else(|| {
            WalError::Config("storage backend returns no ETags; CAS is impossible".into())
        }),
    }
}

pub async fn get(store: &Store, node: &str) -> Result<Option<VersionedLease>, WalError> {
    let key = keys::node_lease(node);
    let Some((bytes, etag)) = store.get(&key).await? else {
        return Ok(None);
    };
    let record = parse(&key, &bytes)?;
    let etag = etag_or_head(store, &key, etag).await?;
    Ok(Some(VersionedLease { record, etag }))
}

/// One LIST of `nodes/`: `(node, etag)` for every lease. ETags let an observer tell whether
/// a lease changed without reading it.
pub async fn list(store: &Store) -> Result<Vec<(String, Option<String>)>, WalError> {
    Ok(store
        .list(keys::NODES_PREFIX)
        .await?
        .into_iter()
        .filter_map(|(k, meta)| keys::lease_node(&k).map(|n| (n.to_string(), meta.e_tag)))
        .collect())
}

/// Declare `observed` dead, conditioned on its lease not having changed since it was
/// read. `Ok(false)` if it changed: the node renewed, so it is alive.
pub async fn tombstone(
    store: &Store,
    observed: &VersionedLease,
    by: &str,
) -> Result<bool, WalError> {
    let mut next = observed.record.clone();
    next.state = LeaseState::Dead;
    next.dead_by = Some(by.to_string());
    let body = Bytes::from(serde_json::to_vec(&next)?);
    Ok(matches!(
        store
            .put_if_match(
                &keys::node_lease(&observed.record.node),
                &observed.etag,
                body
            )
            .await?,
        CasOutcome::Written(_)
    ))
}

#[derive(Debug, Clone)]
pub struct LeaseConfig {
    pub interval: Duration,
    pub timeout: Duration,
    pub safety_margin: Duration,
    /// Refuse to start while another live process renews this node id's lease.
    pub startup_check: bool,
}

impl LeaseConfig {
    pub fn from_core(c: &valka_core::ClusterConfig) -> Self {
        Self {
            interval: Duration::from_millis(c.lease_interval_ms.max(10)),
            timeout: Duration::from_millis(c.lease_timeout_ms.max(20)),
            safety_margin: Duration::from_millis(c.lease_safety_margin_ms),
            startup_check: c.startup_lease_check,
        }
    }

    /// How long a node may go without a successful renewal before it stops serving.
    pub fn serve_window(&self) -> Duration {
        self.timeout.saturating_sub(self.safety_margin)
    }
}

/// Addresses and version advertised in the lease.
#[derive(Debug, Clone, Default)]
pub struct NodeAdvert {
    pub grpc_addr: Option<String>,
    pub http_addr: Option<String>,
    pub internal_addr: Option<String>,
    pub version: String,
}

/// After every segment PUT, before releasing acks. Implementations must not consult a clock.
#[async_trait]
pub trait FenceCheck: Send + Sync {
    async fn verify(&self) -> Result<(), WalError>;
}

/// Trusts itself unconditionally. Only for writer unit tests.
pub struct NoFence;

#[async_trait]
impl FenceCheck for NoFence {
    async fn verify(&self) -> Result<(), WalError> {
        Ok(())
    }
}

struct Current {
    record: LeaseRecord,
    etag: String,
}

/// Owns this process's lease: start, renewal, updates, self-fencing, flush checks.
pub struct LeaseKeeper {
    store: Store,
    key: String,
    node: String,
    inc: u32,
    cfg: LeaseConfig,
    current: Mutex<Current>,
    /// Serialises every CAS this process makes on its lease.
    write_lock: tokio::sync::Mutex<()>,
    /// When the last successful renewal was sent.
    last_ok: Mutex<Instant>,
    /// Set once, when this process learns it is fenced. The node must exit.
    fenced: watch::Sender<Option<String>>,
    serving: watch::Sender<bool>,
}

impl std::fmt::Debug for LeaseKeeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaseKeeper")
            .field("node", &self.node)
            .field("inc", &self.inc)
            .finish()
    }
}

impl LeaseKeeper {
    /// Take this node id's lease with a fresh incarnation. Returns the keeper and the
    /// lease as it was before (the previous incarnation, if any).
    pub async fn start(
        store: Store,
        node: &str,
        advert: NodeAdvert,
        cfg: LeaseConfig,
    ) -> Result<(Arc<Self>, Option<LeaseRecord>), WalError> {
        let key = keys::node_lease(node);
        let mut checked = false;
        loop {
            let old = get(&store, node).await?;
            if let Some(o) = &old
                && cfg.startup_check
                && !checked
                && o.record.state != LeaseState::Dead
            {
                // A live process renews within `interval`. Watch for longer than that.
                tokio::time::sleep(cfg.interval + cfg.interval / 2).await;
                checked = true;
                if let Some(again) = get(&store, node).await?
                    && again.etag != o.etag
                    && again.record.inc == o.record.inc
                    && again.record.state != LeaseState::Dead
                {
                    return Err(WalError::Config(format!(
                        "node id `{node}` is in use by a live process (incarnation {}); \
                         give this node another VALKA_NODE_ID",
                        o.record.inc
                    )));
                }
                continue;
            }
            let mut next = LeaseRecord {
                node: node.to_string(),
                inc: 1,
                state: LeaseState::Alive,
                counter: 0,
                trunc: 1,
                unsplit: Vec::new(),
                grpc_addr: advert.grpc_addr.clone(),
                http_addr: advert.http_addr.clone(),
                internal_addr: advert.internal_addr.clone(),
                version: advert.version.clone(),
                protocol: PROTOCOL,
                started_at: Utc::now(),
                dead_by: None,
            };
            if let Some(o) = &old {
                next.inc = o.record.inc + 1;
                next.unsplit = o.record.incarnations();
            }
            let body = Bytes::from(serde_json::to_vec(&next)?);
            let sent = Instant::now();
            let written = match &old {
                None => store.put_create(&key, body).await?,
                Some(o) => match store.put_if_match(&key, &o.etag, body).await? {
                    CasOutcome::Written(e) => Some(e),
                    CasOutcome::Conflict => None,
                },
            };
            let Some(etag) = written else {
                debug!(node, "lease changed while starting; retrying");
                continue;
            };
            let etag = etag_or_head(&store, &key, etag).await?;
            info!(
                node,
                inc = next.inc,
                unsplit = next.unsplit.len(),
                "lease taken"
            );
            let prev = old.map(|o| o.record);
            let keeper = Arc::new(Self {
                store,
                key,
                node: node.to_string(),
                inc: next.inc,
                cfg,
                current: Mutex::new(Current { record: next, etag }),
                write_lock: tokio::sync::Mutex::new(()),
                last_ok: Mutex::new(sent),
                fenced: watch::Sender::new(None),
                serving: watch::Sender::new(true),
            });
            return Ok((keeper, prev));
        }
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    pub fn inc(&self) -> u32 {
        self.inc
    }

    pub fn config(&self) -> &LeaseConfig {
        &self.cfg
    }

    pub fn record(&self) -> LeaseRecord {
        self.current.lock().record.clone()
    }

    /// Why this process is fenced, once it is.
    pub fn fenced(&self) -> Option<String> {
        self.fenced.borrow().clone()
    }

    pub fn fence_watch(&self) -> watch::Receiver<Option<String>> {
        self.fenced.subscribe()
    }

    /// Renewed recently enough that nobody can have taken over: safe to dispatch and serve.
    pub fn is_serving(&self) -> bool {
        self.fenced().is_none() && self.last_ok.lock().elapsed() < self.cfg.serve_window()
    }

    pub fn serving_watch(&self) -> watch::Receiver<bool> {
        self.serving.subscribe()
    }

    /// Time since the last successful renewal was sent.
    pub fn lease_age(&self) -> Duration {
        self.last_ok.lock().elapsed()
    }

    fn fence(&self, why: String) {
        if self.fenced.borrow().is_none() {
            error!(node = %self.node, inc = self.inc, reason = %why, "lease lost: this process is fenced");
            self.fenced.send_replace(Some(why));
            self.serving.send_replace(false);
        }
    }

    /// Write `change(record)` with `counter + 1`. Returns once the write is in the bucket.
    /// Fails (and fences) if the lease no longer belongs to this incarnation.
    pub async fn update(&self, change: impl Fn(&mut LeaseRecord)) -> Result<(), WalError> {
        if let Some(why) = self.fenced() {
            return Err(WalError::Fenced(why));
        }
        let _w = self.write_lock.lock().await;
        for _ in 0..8 {
            let (mut next, etag) = {
                let c = self.current.lock();
                (c.record.clone(), c.etag.clone())
            };
            next.counter += 1;
            change(&mut next);
            let body = Bytes::from(serde_json::to_vec(&next)?);
            let sent = Instant::now();
            match self.store.put_if_match(&self.key, &etag, body).await? {
                CasOutcome::Written(new) => {
                    let etag = etag_or_head(&self.store, &self.key, new).await?;
                    *self.current.lock() = Current { record: next, etag };
                    *self.last_ok.lock() = sent;
                    self.serving
                        .send_if_modified(|s| !std::mem::replace(s, true));
                    return Ok(());
                }
                CasOutcome::Conflict => {
                    // Either an earlier write of ours landed with its response lost, or
                    // someone else wrote the lease. Only the former lets us continue.
                    match get(&self.store, &self.node).await? {
                        Some(v)
                            if v.record.inc == self.inc && v.record.state != LeaseState::Dead =>
                        {
                            debug!(node = %self.node, "lease conflict was our own landed write");
                            *self.current.lock() = Current {
                                record: v.record,
                                etag: v.etag,
                            };
                        }
                        Some(v) => {
                            let why = format!(
                                "lease now held by incarnation {} in state {:?} (dead_by {:?})",
                                v.record.inc, v.record.state, v.record.dead_by
                            );
                            self.fence(why.clone());
                            return Err(WalError::Fenced(why));
                        }
                        None => {
                            let why = "lease object deleted".to_string();
                            self.fence(why.clone());
                            return Err(WalError::Fenced(why));
                        }
                    }
                }
            }
        }
        Err(WalError::NotDurable("lease kept changing under us".into()))
    }

    /// Renew once (bump the counter).
    pub async fn renew(&self) -> Result<(), WalError> {
        self.update(|_| {}).await
    }

    /// Raise the truncation watermark. Segments below `seq` may be deleted once this returns.
    pub async fn raise_trunc(&self, seq: u64) -> Result<(), WalError> {
        if self.current.lock().record.trunc >= seq {
            return Ok(());
        }
        self.update(|r| r.trunc = r.trunc.max(seq)).await
    }

    /// Forget an older incarnation once its log is split.
    pub async fn drop_unsplit(&self, inc: u32) -> Result<(), WalError> {
        if !self
            .current
            .lock()
            .record
            .unsplit
            .iter()
            .any(|u| u.inc == inc)
        {
            return Ok(());
        }
        self.update(|r| r.unsplit.retain(|u| u.inc != inc)).await
    }

    pub async fn set_state(&self, state: LeaseState) -> Result<(), WalError> {
        let me = self.node.clone();
        self.update(|r| {
            r.state = state;
            if state == LeaseState::Dead {
                r.dead_by = Some(me.clone());
            }
        })
        .await
    }

    /// Renewal loop. Exits when fenced or when `shutdown` turns true.
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut tick = tokio::time::interval(self.cfg.interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;
        loop {
            tokio::select! {
                r = shutdown.changed() => { if r.is_err() || *shutdown.borrow() { return; } }
                _ = tick.tick() => {}
            }
            if self.fenced().is_some() {
                return;
            }
            match self.renew().await {
                Ok(()) => {}
                Err(WalError::Fenced(_)) => return,
                Err(e) => {
                    warn!(node = %self.node, error = %e, age_ms = self.lease_age().as_millis() as u64, "lease renewal failed")
                }
            }
            let serving = self.is_serving();
            self.serving.send_if_modified(|s| {
                if *s != serving {
                    if !serving {
                        warn!(node = %self.node, "lease too old: not serving until it renews");
                    }
                    *s = serving;
                    true
                } else {
                    false
                }
            });
        }
    }
}

#[async_trait]
impl FenceCheck for LeaseKeeper {
    async fn verify(&self) -> Result<(), WalError> {
        if let Some(why) = self.fenced() {
            return Err(WalError::Fenced(why));
        }
        let (etag, counter) = {
            let c = self.current.lock();
            (c.etag.clone(), c.record.counter)
        };
        match self.store.get_if_none_match(&self.key, &etag).await? {
            Conditional::NotModified => Ok(()),
            Conditional::Modified { bytes, etag } => {
                let rec = parse(&self.key, &bytes)?;
                if rec.inc == self.inc && rec.state != LeaseState::Dead {
                    // A renewal of ours that this check raced with. Keep the newest view.
                    if rec.counter > counter
                        && let Some(etag) = etag
                    {
                        let mut c = self.current.lock();
                        if rec.counter > c.record.counter {
                            *c = Current { record: rec, etag };
                        }
                    }
                    Ok(())
                } else {
                    let why = format!(
                        "lease now held by incarnation {} in state {:?}",
                        rec.inc, rec.state
                    );
                    self.fence(why.clone());
                    Err(WalError::Fenced(why))
                }
            }
            Conditional::Missing => {
                let why = "lease object deleted".to_string();
                self.fence(why.clone());
                Err(WalError::Fenced(why))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LeaseConfig {
        LeaseConfig {
            interval: Duration::from_millis(100),
            timeout: Duration::from_millis(1000),
            safety_margin: Duration::from_millis(300),
            startup_check: false,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn start_bumps_incarnation_and_carries_unsplit() {
        let s = Store::memory();
        let (a, prev) = LeaseKeeper::start(s.clone(), "n", NodeAdvert::default(), cfg())
            .await
            .unwrap();
        assert!(prev.is_none());
        assert_eq!(a.inc(), 1);
        a.raise_trunc(7).await.unwrap();
        let (b, prev) = LeaseKeeper::start(s.clone(), "n", NodeAdvert::default(), cfg())
            .await
            .unwrap();
        assert_eq!(b.inc(), 2);
        assert_eq!(prev.unwrap().inc, 1);
        assert_eq!(b.record().unsplit, vec![UnsplitInc { inc: 1, trunc: 7 }]);

        // The old incarnation learns it is fenced on its next write and on its next check.
        assert!(matches!(a.renew().await, Err(WalError::Fenced(_))));
        assert!(a.fenced().is_some());
        assert!(!a.is_serving());
        b.verify().await.unwrap();
        b.drop_unsplit(1).await.unwrap();
        assert!(b.record().unsplit.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn flush_check_fails_after_tombstone() {
        let s = Store::memory();
        let (a, _) = LeaseKeeper::start(s.clone(), "n", NodeAdvert::default(), cfg())
            .await
            .unwrap();
        a.verify().await.unwrap();
        a.renew().await.unwrap();
        a.verify().await.unwrap();
        let observed = get(&s, "n").await.unwrap().unwrap();
        assert!(tombstone(&s, &observed, "guardian").await.unwrap());
        assert!(matches!(a.verify().await, Err(WalError::Fenced(_))));
        assert!(a.fenced().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn tombstone_loses_to_a_renewal() {
        let s = Store::memory();
        let (a, _) = LeaseKeeper::start(s.clone(), "n", NodeAdvert::default(), cfg())
            .await
            .unwrap();
        let observed = get(&s, "n").await.unwrap().unwrap();
        a.renew().await.unwrap();
        assert!(!tombstone(&s, &observed, "guardian").await.unwrap());
        a.verify().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn stale_lease_stops_serving_until_renewed() {
        let (s, faults) = crate::fault::faulty_memory_store(1);
        let (a, _) = LeaseKeeper::start(s.clone(), "n", NodeAdvert::default(), cfg())
            .await
            .unwrap();
        let (_tx, rx) = watch::channel(false);
        tokio::spawn(a.clone().run(rx));
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(a.is_serving());
        faults.set_puts_down(true);
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(
            !a.is_serving(),
            "no renewal for longer than timeout - margin"
        );
        assert!(a.fenced().is_none(), "stale is not fenced");
        faults.set_puts_down(false);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(a.is_serving());
    }

    #[tokio::test(start_paused = true)]
    async fn startup_check_refuses_a_live_duplicate() {
        let s = Store::memory();
        let (a, _) = LeaseKeeper::start(s.clone(), "n", NodeAdvert::default(), cfg())
            .await
            .unwrap();
        let (_tx, rx) = watch::channel(false);
        tokio::spawn(a.clone().run(rx));
        let mut c = cfg();
        c.startup_check = true;
        let err = LeaseKeeper::start(s.clone(), "n", NodeAdvert::default(), c.clone())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("in use"), "{err}");
        // Once the first process is gone, the same id starts after one observation.
        a.set_state(LeaseState::Alive).await.unwrap();
        a.fence("test".into());
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (b, _) = LeaseKeeper::start(s, "n", NodeAdvert::default(), c)
            .await
            .unwrap();
        assert_eq!(b.inc(), 2);
    }
}
