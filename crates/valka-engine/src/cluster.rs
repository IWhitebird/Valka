//! The control plane every node runs, alone or in a cluster: membership, placement, the
//! guardian (death detection, tombstones, splits) and the garbage collector.
//!
//! - **Membership** comes from the bucket: one LIST of `nodes/` per `membership_interval`
//!   gives every lease's ETag; a lease whose ETag has not changed for `lease_timeout` by
//!   *this* node's monotonic clock is suspect. No two clocks are ever compared.
//! - **Placement** is leaderless rendezvous hashing over eligible nodes. Every move is a
//!   CAS on the shard's owner record, so nodes that briefly disagree are still safe.
//! - **Guardian** of a suspect node X is the first live node in rendezvous order for X's
//!   id. It tombstones X's lease (CAS on the ETag it watched) and splits X's logs.
//! - **Garbage collection** runs on the desired owner of shard 0.

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, info, warn};
use valka_core::{ClusterConfig, ShardId, rendezvous_pick};
use valka_wal::lease::{self, LeaseRecord, LeaseState, VersionedLease};
use valka_wal::lsn::keys;
use valka_wal::owners;
use valka_wal::snapshot::{self, LogPtr};

use crate::engine::Engine;
use crate::state::ShardPhase;

/// What this node knows about another node.
#[derive(Debug, Clone)]
struct Member {
    lease: Option<LeaseRecord>,
    etag: Option<String>,
    /// When this node first saw the current ETag.
    etag_since: Instant,
    /// When this node first saw the lease in state Dead.
    dead_since: Option<Instant>,
    fetched_at: Option<Instant>,
}

/// A row of the membership view, for operators.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MemberInfo {
    pub node_id: String,
    pub inc: u32,
    pub state: String,
    pub live: bool,
    pub suspect: bool,
    pub eligible: bool,
    pub lease_age_ms: u64,
    pub grpc_addr: Option<String>,
    pub http_addr: Option<String>,
    pub internal_addr: Option<String>,
    pub version: String,
    pub protocol: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub unsplit: Vec<u32>,
}

pub(crate) struct Membership {
    me: String,
    lease_timeout: Duration,
    join_delay: Duration,
    split_timeout: Duration,
    members: Mutex<BTreeMap<String, Member>>,
    last_refresh: Mutex<Option<Instant>>,
    /// Shards that were not claimable: next attempt, and the current backoff.
    backoff: Mutex<HashMap<ShardId, (Instant, Duration)>>,
    last_gc: Mutex<Option<Instant>>,
}

impl Membership {
    pub(crate) fn new(me: &str, cfg: &ClusterConfig) -> Self {
        Self {
            me: me.to_string(),
            lease_timeout: Duration::from_millis(cfg.lease_timeout_ms.max(1)),
            join_delay: Duration::from_millis(cfg.join_delay_ms),
            split_timeout: Duration::from_millis(cfg.split_timeout_ms.max(1)),
            members: Mutex::new(BTreeMap::new()),
            last_refresh: Mutex::new(None),
            backoff: Mutex::new(HashMap::new()),
            last_gc: Mutex::new(None),
        }
    }

    fn live(&self, m: &Member, now: Instant) -> bool {
        m.lease
            .as_ref()
            .is_some_and(|l| l.state != LeaseState::Dead)
            && now.duration_since(m.etag_since) < self.lease_timeout
    }

    fn suspect(&self, m: &Member, now: Instant) -> bool {
        m.lease
            .as_ref()
            .is_some_and(|l| l.state != LeaseState::Dead)
            && now.duration_since(m.etag_since) >= self.lease_timeout
    }
}

impl Engine {
    /// Re-read the node list from the bucket (one LIST, plus a GET per changed lease).
    pub async fn refresh_membership(&self) {
        let store = &self.inner.store;
        let listing = match lease::list(store).await {
            Ok(l) => l,
            Err(e) => {
                warn!(error = %e, "membership: listing leases failed");
                return;
            }
        };
        let now = Instant::now();
        let ms = &self.inner.membership;
        let mut fetch = Vec::new();
        {
            let mut members = ms.members.lock();
            let seen: BTreeSet<&str> = listing.iter().map(|(n, _)| n.as_str()).collect();
            members.retain(|n, _| seen.contains(n.as_str()));
            for (node, etag) in &listing {
                if *node == ms.me {
                    continue;
                }
                let m = members.entry(node.clone()).or_insert_with(|| Member {
                    lease: None,
                    etag: None,
                    etag_since: now,
                    dead_since: None,
                    fetched_at: None,
                });
                let changed = m.etag != *etag || etag.is_none();
                if m.etag != *etag {
                    m.etag = etag.clone();
                    m.etag_since = now;
                }
                if changed || m.lease.is_none() {
                    fetch.push(node.clone());
                }
            }
        }
        for node in fetch {
            match lease::get(store, &node).await {
                Ok(Some(v)) => self.observe_lease(v, now),
                Ok(None) => {}
                Err(e) => debug!(node, error = %e, "membership: reading lease failed"),
            }
        }
        *ms.last_refresh.lock() = Some(now);
    }

    fn observe_lease(&self, v: VersionedLease, now: Instant) {
        let ms = &self.inner.membership;
        if v.record.node == ms.me {
            return;
        }
        let mut members = ms.members.lock();
        let m = members
            .entry(v.record.node.clone())
            .or_insert_with(|| Member {
                lease: None,
                etag: None,
                etag_since: now,
                dead_since: None,
                fetched_at: None,
            });
        if m.etag.as_deref() != Some(v.etag.as_str()) {
            m.etag = Some(v.etag);
            m.etag_since = now;
        }
        if v.record.state == LeaseState::Dead {
            m.dead_since.get_or_insert(now);
        } else {
            m.dead_since = None;
        }
        m.lease = Some(v.record);
        m.fetched_at = Some(now);
    }

    /// Nodes that take part in placement, this one included when it may serve.
    pub fn eligible_nodes(&self) -> Vec<String> {
        let ms = &self.inner.membership;
        let now = Instant::now();
        let wall = self.inner.clock.now();
        let join = chrono::Duration::from_std(ms.join_delay).unwrap_or_default();
        let mut cands: Vec<(String, bool)> = Vec::new();
        let me = self.inner.lease.record();
        if me.state == LeaseState::Alive && self.is_serving() {
            cands.push((ms.me.clone(), wall - me.started_at >= join));
        }
        for (node, m) in ms.members.lock().iter() {
            if !ms.live(m, now) {
                continue;
            }
            let l = m.lease.as_ref().expect("live implies a lease");
            if l.state != LeaseState::Alive {
                continue;
            }
            cands.push((node.clone(), wall - l.started_at >= join));
        }
        // Newcomers wait out the join delay, unless nobody is past it (a cold start).
        let any_settled = cands.iter().any(|(_, settled)| *settled);
        cands
            .into_iter()
            .filter(|(_, settled)| *settled || !any_settled)
            .map(|(n, _)| n)
            .collect()
    }

    /// Where `shard` should live given `eligible` nodes.
    pub fn desired_owner<'a>(&self, shard: ShardId, eligible: &'a [String]) -> Option<&'a str> {
        rendezvous_pick(&shard.0.to_be_bytes(), eligible.iter().map(String::as_str))
    }

    /// Live nodes other than this one.
    pub fn live_peers(&self) -> Vec<(String, LeaseRecord)> {
        let ms = &self.inner.membership;
        let now = Instant::now();
        ms.members
            .lock()
            .iter()
            .filter(|(_, m)| ms.live(m, now))
            .filter_map(|(n, m)| m.lease.clone().map(|l| (n.clone(), l)))
            .collect()
    }

    /// Everything this node knows about the cluster's nodes, itself included.
    pub fn members(&self) -> Vec<MemberInfo> {
        let ms = &self.inner.membership;
        let now = Instant::now();
        let eligible: BTreeSet<String> = self.eligible_nodes().into_iter().collect();
        let mut out = Vec::new();
        let me = self.inner.lease.record();
        out.push(MemberInfo {
            node_id: ms.me.clone(),
            inc: me.inc,
            state: format!("{:?}", me.state).to_lowercase(),
            live: self.is_serving(),
            suspect: false,
            eligible: eligible.contains(&ms.me),
            lease_age_ms: self.inner.lease.lease_age().as_millis() as u64,
            grpc_addr: me.grpc_addr.clone(),
            http_addr: me.http_addr.clone(),
            internal_addr: me.internal_addr.clone(),
            version: me.version.clone(),
            protocol: me.protocol,
            started_at: Some(me.started_at),
            unsplit: me.unsplit.iter().map(|u| u.inc).collect(),
        });
        for (node, m) in ms.members.lock().iter() {
            let Some(l) = &m.lease else { continue };
            out.push(MemberInfo {
                node_id: node.clone(),
                inc: l.inc,
                state: format!("{:?}", l.state).to_lowercase(),
                live: ms.live(m, now),
                suspect: ms.suspect(m, now),
                eligible: eligible.contains(node),
                lease_age_ms: now.duration_since(m.etag_since).as_millis() as u64,
                grpc_addr: l.grpc_addr.clone(),
                http_addr: l.http_addr.clone(),
                internal_addr: l.internal_addr.clone(),
                version: l.version.clone(),
                protocol: l.protocol,
                started_at: Some(l.started_at),
                unsplit: l.incarnations().iter().map(|u| u.inc).collect(),
            });
        }
        out
    }

    // ───────────────────────── placement ─────────────────────────

    /// One placement round: claim shards placement gives this node, hand over shards it
    /// should not hold. Returns the number of shards claimed.
    pub async fn placement_round(&self) -> usize {
        if !self.is_serving() {
            return 0;
        }
        let eligible = self.eligible_nodes();
        let me = self.node_id().to_string();
        let now = Instant::now();
        let draining = self.inner.lease.record().state != LeaseState::Alive;

        let mut claims = Vec::new();
        let mut releases = Vec::new();
        {
            let backoff = self.inner.membership.backoff.lock();
            let residency = Duration::from_secs(self.inner.cfg.cluster.min_residency_secs);
            let moved = self.inner.moved_at.lock();
            for (i, m) in self.inner.shards.iter().enumerate() {
                let shard = ShardId(i as u16);
                let phase = m.lock().phase;
                let desired = self.desired_owner(shard, &eligible);
                match phase {
                    ShardPhase::Unowned
                        if !draining
                            && desired == Some(me.as_str())
                            && backoff.get(&shard).is_none_or(|(at, _)| now >= *at) =>
                    {
                        claims.push(shard)
                    }
                    ShardPhase::Serving if draining => releases.push(shard),
                    ShardPhase::Serving
                        if desired.is_some_and(|d| d != me)
                            && moved
                                .get(&shard)
                                .is_none_or(|at| now.duration_since(*at) >= residency) =>
                    {
                        releases.push(shard)
                    }
                    _ => {}
                }
            }
        }

        let mut claimed = 0;
        if !claims.is_empty() {
            claimed = self.claim_shards(claims.clone()).await;
            let mut backoff = self.inner.membership.backoff.lock();
            for s in claims {
                if self.owns(s) {
                    backoff.remove(&s);
                } else {
                    let base = Duration::from_millis(self.inner.cfg.cluster.placement_interval_ms);
                    let next = backoff
                        .get(&s)
                        .map_or(base, |(_, d)| (*d * 2).min(Duration::from_secs(30)));
                    backoff.insert(s, (now + next, next));
                }
            }
        }
        if !releases.is_empty() {
            let limit = if draining {
                usize::MAX
            } else {
                self.inner.cfg.cluster.handover_per_tick.max(1)
            };
            self.release_shards(releases, limit).await;
        }
        claimed
    }

    // ───────────────────────── guardian ─────────────────────────

    /// Tombstone suspects this node guards, and split logs of dead nodes.
    pub async fn guardian_round(&self) {
        if !self.is_serving() {
            return;
        }
        let ms = &self.inner.membership;
        let now = Instant::now();
        let me = self.node_id().to_string();
        let live: Vec<String> = std::iter::once(me.clone())
            .chain(self.live_peers().into_iter().map(|(n, _)| n))
            .collect();
        let guardian_of = |node: &str| -> bool {
            let others: Vec<&str> = live
                .iter()
                .map(String::as_str)
                .filter(|n| *n != node)
                .collect();
            rendezvous_pick(node.as_bytes(), others) == Some(me.as_str())
        };
        let snapshot: Vec<(String, Member)> = ms
            .members
            .lock()
            .iter()
            .map(|(n, m)| (n.clone(), m.clone()))
            .collect();
        for (node, m) in snapshot {
            let Some(l) = &m.lease else { continue };
            if ms.suspect(&m, now) && guardian_of(&node) {
                match lease::get(&self.inner.store, &node).await {
                    Ok(Some(v)) if Some(&v.etag) == m.etag.as_ref() => {
                        match lease::tombstone(&self.inner.store, &v, &me).await {
                            Ok(true) => {
                                warn!(node, inc = v.record.inc, "lease expired: tombstoned");
                                let mut dead = v.clone();
                                dead.record.state = LeaseState::Dead;
                                dead.record.dead_by = Some(me.clone());
                                if let Ok(Some(fresh)) = lease::get(&self.inner.store, &node).await
                                {
                                    self.observe_lease(fresh, now);
                                }
                                self.split_node(&dead.record).await;
                            }
                            Ok(false) => debug!(node, "tombstone lost: the node renewed"),
                            Err(e) => warn!(node, error = %e, "tombstone failed"),
                        }
                    }
                    Ok(Some(v)) => self.observe_lease(v, now),
                    Ok(None) => {}
                    Err(e) => debug!(node, error = %e, "guardian: reading lease failed"),
                }
                continue;
            }
            if l.state == LeaseState::Dead {
                let overdue = m
                    .dead_since
                    .is_some_and(|d| now.duration_since(d) >= ms.split_timeout);
                if guardian_of(&node) || overdue {
                    self.split_node(l).await;
                }
            }
        }
    }

    /// Split every incarnation of a dead node that is not split yet.
    async fn split_node(&self, dead: &LeaseRecord) {
        for u in dead.incarnations() {
            if let Err(e) = self.split_incarnation(&dead.node, u.inc, u.trunc).await {
                warn!(node = %dead.node, inc = u.inc, error = %e, "split failed; will retry");
            }
        }
    }

    // ───────────────────────── loop ─────────────────────────

    pub(crate) async fn control_loop(self) {
        let mut shutdown = self.inner.shutdown.subscribe();
        let c = &self.inner.cfg.cluster;
        let interval = Duration::from_millis(c.placement_interval_ms.max(10));
        let refresh_every = Duration::from_millis(c.membership_interval_ms.max(10));
        let gc_every = Duration::from_secs(c.gc_interval_secs.max(1));
        loop {
            // Jitter keeps nodes from moving in lockstep.
            let jitter = interval.mul_f64(rand::random::<f64>() * 0.2);
            tokio::select! {
                _ = shutdown.changed() => { if *shutdown.borrow() { return; } }
                _ = tokio::time::sleep(interval + jitter) => {}
            }
            if self.poisoned().is_some() {
                return;
            }
            let due = self
                .inner
                .membership
                .last_refresh
                .lock()
                .is_none_or(|t| t.elapsed() >= refresh_every);
            if due {
                self.refresh_membership().await;
            }
            self.guardian_round().await;
            self.placement_round().await;
            let gc_due = self
                .inner
                .membership
                .last_gc
                .lock()
                .is_none_or(|t| t.elapsed() >= gc_every);
            if gc_due {
                *self.inner.membership.last_gc.lock() = Some(Instant::now());
                let eligible = self.eligible_nodes();
                if self.desired_owner(ShardId(0), &eligible) == Some(self.node_id())
                    && let Err(e) = self.gc_round().await
                {
                    warn!(error = %e, "garbage collection failed");
                }
            }
        }
    }

    // ───────────────────────── garbage collection ─────────────────────────

    /// Delete what no future replay or claim can need (docs/wal/PHASE2.md §8.7):
    /// logs of split incarnations, snapshots beyond the newest few per shard, split
    /// markers nothing references, long-dead leases, self-test leftovers.
    pub async fn gc_round(&self) -> Result<GcReport, valka_core::ServerError> {
        let store = &self.inner.store;
        let mut report = GcReport::default();
        let keep = self.inner.cfg.wal.snapshots_to_keep.max(1);

        // Split markers and the incarnations they cover.
        let markers: BTreeSet<(String, u32)> = store
            .list(keys::SPLITS_PREFIX)
            .await?
            .into_iter()
            .filter_map(|(k, _)| keys::parse_split_marker(&k).map(|(n, i)| (n.to_string(), i)))
            .collect();

        // 1. Logs of split incarnations.
        let mut logs: BTreeMap<(String, u32), Vec<String>> = BTreeMap::new();
        for (k, _) in store.list(keys::WAL_PREFIX).await? {
            if let Some((node, lsn)) = keys::parse_segment(&k) {
                logs.entry((node.to_string(), lsn.inc)).or_default().push(k);
            }
        }
        for (inc_id, segs) in &logs {
            if markers.contains(inc_id) {
                for k in segs {
                    store.delete(k).await?;
                    report.segments_deleted += 1;
                }
            }
        }

        // 2. Snapshots beyond the newest `keep` per shard, and the incarnations the
        //    remaining ones point into.
        let mut referenced: BTreeSet<(String, u32)> = BTreeSet::new();
        for (_, snaps) in snapshot::list_all(store).await? {
            report.snapshots_deleted += snapshot::prune_listed(store, &snaps, keep).await?;
            for k in snaps.iter().rev().take(keep) {
                if let LogPtr::At { node, inc, .. } = &k.ptr {
                    referenced.insert((node.clone(), *inc));
                }
            }
        }

        // 3. Markers whose log is gone and that no snapshot points into, unless an owner
        //    record still names that incarnation (a claim needs the marker).
        let candidates: Vec<&(String, u32)> = markers
            .iter()
            .filter(|m| !logs.contains_key(*m) && !referenced.contains(*m))
            .collect();
        let mut owner_refs: BTreeSet<(String, u32)> = BTreeSet::new();
        let mut owner_nodes: BTreeSet<String> = BTreeSet::new();
        let any_dead_lease = {
            let ms = self.inner.membership.members.lock();
            ms.values().any(|m| {
                m.lease
                    .as_ref()
                    .is_some_and(|l| l.state == LeaseState::Dead)
            })
        };
        if !candidates.is_empty() || any_dead_lease {
            for (_, v) in owners::list_all(store).await? {
                owner_refs.insert((v.record.node.clone(), v.record.inc));
                owner_nodes.insert(v.record.node);
            }
        }
        for m in candidates {
            if !owner_refs.contains(m) {
                store.delete(&keys::split_marker(&m.0, m.1)).await?;
                report.markers_deleted += 1;
            }
        }

        // 4. Leases of nodes dead for a long time that nothing refers to any more.
        let long = Duration::from_secs(3600).max(self.inner.membership.split_timeout * 4);
        let now = Instant::now();
        let dead: Vec<String> = {
            let ms = self.inner.membership.members.lock();
            ms.iter()
                .filter(|(_, m)| m.dead_since.is_some_and(|d| now.duration_since(d) >= long))
                .map(|(n, _)| n.clone())
                .collect()
        };
        for node in dead {
            if owner_nodes.contains(&node) || logs.keys().any(|(n, _)| *n == node) {
                continue;
            }
            if let Some(v) = lease::get(store, &node).await?
                && v.record.state == LeaseState::Dead
                && v.record.incarnations().iter().all(|u| {
                    markers.contains(&(node.clone(), u.inc))
                        || !logs.contains_key(&(node.clone(), u.inc))
                })
            {
                lease::delete_if_unchanged(store, &v).await?;
                report.leases_deleted += 1;
            }
        }

        // 5. Self-test probes left behind by crashed starts.
        for (k, meta) in store.list("selftest/").await? {
            if Utc::now() - meta.last_modified > chrono::Duration::hours(1) {
                store.delete(&k).await?;
            }
        }
        if report != GcReport::default() {
            info!(?report, "garbage collection");
        }
        Ok(report)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GcReport {
    pub segments_deleted: usize,
    pub snapshots_deleted: usize,
    pub markers_deleted: usize,
    pub leases_deleted: usize,
}
