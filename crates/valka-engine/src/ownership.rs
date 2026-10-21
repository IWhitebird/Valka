//! Moving shards: claim, release, and splitting a dead incarnation's log.
//!
//! The protocol (docs/wal/PHASE2.md §8):
//! - **Claim** S: allowed when S's owner record is missing, released, or names an
//!   incarnation that is dead *and* whose log is split. CAS the record to
//!   `(me, my inc, gen + 1)`, load S's newest snapshot, write an acquisition snapshot that
//!   points into my log, and only then serve.
//! - **Release** S: stop accepting commands, wait until S's records are durable, write a
//!   final snapshot, CAS the record to released, drop the state.
//! - **Split** a dead incarnation D: read D's log to its sealed end, rebuild every shard
//!   D wrote from its newest snapshot at that generation plus D's records, write those
//!   as split snapshots, then write D's split marker. A dead log is read once, by one node.

use futures::StreamExt;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;
use tracing::{debug, error, info, warn};
use valka_core::{ServerError, ShardId};
use valka_wal::lease::{self, LeaseRecord};
use valka_wal::owners::{self, OwnerRecord, OwnerState, Versioned};
use valka_wal::snapshot::{self, LogPtr, SnapshotKey};
use valka_wal::split::{self, SplitMarker};
use valka_wal::{Envelope, WalError};

use crate::engine::Engine;
use crate::state::{ShardPhase, ShardSnapshot, ShardState};

/// Why a shard could not be claimed now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimRefused {
    /// Its owner is alive (or its log is not split yet): try again later.
    Busy(String),
    /// Someone else moved it first.
    Lost,
    /// The bucket is inconsistent with the protocol. Alert.
    Invariant(String),
}

/// Facts about other nodes' incarnations, cached for one claim round.
#[derive(Default)]
pub(crate) struct IncCache {
    leases: HashMap<String, Option<LeaseRecord>>,
    split: HashMap<(String, u32), bool>,
}

impl IncCache {
    async fn lease(
        &mut self,
        store: &valka_wal::Store,
        node: &str,
    ) -> Result<Option<LeaseRecord>, WalError> {
        if let Some(l) = self.leases.get(node) {
            return Ok(l.clone());
        }
        let l = lease::get(store, node).await?.map(|v| v.record);
        self.leases.insert(node.to_string(), l.clone());
        Ok(l)
    }

    /// `(node, inc)` can no longer write and its log is fully split.
    async fn dead_and_split(
        &mut self,
        store: &valka_wal::Store,
        node: &str,
        inc: u32,
    ) -> Result<bool, WalError> {
        let dead = match self.lease(store, node).await? {
            Some(l) => l.is_dead_incarnation(inc),
            // A missing lease belongs to a node that was garbage-collected: long dead.
            None => true,
        };
        if !dead {
            return Ok(false);
        }
        let k = (node.to_string(), inc);
        if let Some(s) = self.split.get(&k) {
            return Ok(*s);
        }
        let s = split::marker_exists(store, node, inc).await?;
        // Only cache a positive answer: a split may finish while the round runs.
        if s {
            self.split.insert(k, true);
        }
        Ok(s)
    }
}

impl Engine {
    // ───────────────────────── claim ─────────────────────────

    /// Claim `shards` (at most `claim_concurrency` at once). Returns how many are now
    /// served. Shards that cannot be claimed yet are skipped.
    pub async fn claim_shards(&self, shards: Vec<ShardId>) -> usize {
        if shards.is_empty() || !self.is_serving() {
            return 0;
        }
        let _moves = self.inner.moves.lock().await;
        let cache = tokio::sync::Mutex::new(IncCache::default());
        let concurrency = self.inner.cfg.cluster.claim_concurrency.max(1);
        let results: Vec<(ShardId, Result<(), ClaimRefused>)> = futures::stream::iter(shards)
            .map(|s| {
                let cache = &cache;
                async move { (s, self.claim_one(s, cache).await) }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;
        let mut claimed = 0;
        for (s, r) in results {
            match r {
                Ok(()) => {
                    claimed += 1;
                    valka_core::metrics::record_shard_claim("claimed");
                }
                Err(ClaimRefused::Busy(why)) => {
                    valka_core::metrics::record_shard_claim("busy");
                    debug!(shard = %s, why, "not claimable yet")
                }
                Err(ClaimRefused::Lost) => {
                    valka_core::metrics::record_shard_claim("lost");
                    debug!(shard = %s, "claim lost to another node")
                }
                Err(ClaimRefused::Invariant(why)) => {
                    valka_core::metrics::record_shard_claim("invariant");
                    error!(shard = %s, why, "refusing to claim: bucket violates the protocol")
                }
            }
        }
        if claimed > 0 {
            self.inner.pending_wake.notify_one();
            info!(claimed, "claimed shards");
        }
        claimed
    }

    async fn claim_one(
        &self,
        shard: ShardId,
        cache: &tokio::sync::Mutex<IncCache>,
    ) -> Result<(), ClaimRefused> {
        {
            let mut st = self.inner.shards[shard.index()].lock();
            if st.phase != ShardPhase::Unowned {
                return Err(ClaimRefused::Busy(format!("phase {}", st.phase.as_str())));
            }
            st.phase = ShardPhase::Loading;
        }
        let r = self.claim_loading(shard, cache).await;
        if r.is_err() {
            let mut st = self.inner.shards[shard.index()].lock();
            if st.phase == ShardPhase::Loading {
                *st = ShardState::new(shard);
            }
        }
        r
    }

    async fn claim_loading(
        &self,
        shard: ShardId,
        cache: &tokio::sync::Mutex<IncCache>,
    ) -> Result<(), ClaimRefused> {
        let store = &self.inner.store;
        let me = self.node_id().to_string();
        let inc = self.incarnation();
        // An unreadable owner record or newest snapshot is not a transient failure: the
        // claim must not go on from older state, and someone has to look.
        let io = |e: WalError| match e {
            WalError::Corrupt(why) => ClaimRefused::Invariant(why),
            e => ClaimRefused::Busy(e.to_string()),
        };

        // 1. Who owns it, and may it move?
        let current = owners::get(store, shard).await.map_err(io)?;
        if let Some(v) = &current {
            let r = &v.record;
            let movable = match r.state {
                OwnerState::Released => true,
                OwnerState::Owned if r.node == me && r.inc == inc => true,
                OwnerState::Owned => cache
                    .lock()
                    .await
                    .dead_and_split(store, &r.node, r.inc)
                    .await
                    .map_err(io)?,
            };
            if !movable {
                // Definitely someone else's: not a claim of ours to finish.
                self.inner.stranded.lock().remove(&shard);
                return Err(ClaimRefused::Busy(format!(
                    "owned by {}.{} (gen {})",
                    r.node, r.inc, r.generation
                )));
            }
        }
        let generation = current.as_ref().map_or(0, |v| v.record.generation) + 1;
        // An earlier claim attempt of this very process that failed after its CAS.
        let own_attempt = current.as_ref().is_some_and(|v| {
            v.record.node == me && v.record.inc == inc && v.record.state == OwnerState::Owned
        });

        // 2. Move its history to me.
        let next = OwnerRecord {
            node: me.clone(),
            inc,
            generation,
            state: OwnerState::Owned,
        };
        // Once the CAS may have landed, the record may name this incarnation: then nobody
        // else can claim the shard while this incarnation lives, so it must finish the
        // claim even if a later step (or the CAS's own response) fails. An ambiguous
        // failure counts: the PUT may have landed with its response lost.
        self.inner.stranded.lock().insert(shard);
        if !owners::cas(store, shard, current.as_ref(), &next)
            .await
            .map_err(io)?
        {
            self.inner.stranded.lock().remove(&shard);
            return Err(ClaimRefused::Lost);
        }

        // 3. Its newest snapshot, listed only now: a split that finished after an earlier
        //    LIST (its marker is what made the shard claimable) wrote newer snapshots.
        //    No snapshot at all: nobody ever served it.
        let listed = snapshot::list(store, shard).await.map_err(io)?;
        let newest = match listed.last() {
            None => None,
            Some(_) => snapshot::load_newest_of::<ShardSnapshot>(store, &listed, |_| true)
                .await
                .map_err(io)?,
        };
        let mut state = match newest {
            None => ShardState::new(shard),
            Some((key, snap)) => {
                if key.generation >= generation {
                    return Err(ClaimRefused::Invariant(format!(
                        "snapshot {} is at or above the generation being claimed ({generation})",
                        key.key()
                    )));
                }
                if let LogPtr::At {
                    node, inc: pinc, ..
                } = &key.ptr
                {
                    let ok = if node == &me && *pinc == inc {
                        own_attempt
                    } else {
                        cache
                            .lock()
                            .await
                            .dead_and_split(store, node, *pinc)
                            .await
                            .map_err(io)?
                    };
                    if !ok {
                        // Unreachable while GC keeps markers that snapshots point at. If
                        // the log is dead but unsplit, its guardian splits it: back off.
                        let dead = match lease::get(store, node).await.map_err(io)? {
                            Some(l) => l.record.is_dead_incarnation(*pinc),
                            None => true,
                        };
                        let why = format!(
                            "newest snapshot {} points into a log that is not split",
                            key.key()
                        );
                        return Err(if dead {
                            error!(shard = %shard, why, "waiting for that log's split");
                            ClaimRefused::Busy(why)
                        } else {
                            ClaimRefused::Invariant(why)
                        });
                    }
                }
                ShardState::from_snapshot(snap, Some(key))
            }
        };
        state.generation = generation;

        // 4. Acquisition snapshot, pointing into my log, before serving.
        let ptr = LogPtr::At {
            node: me.clone(),
            inc,
            seq: self.inner.writer.next_lsn().seq,
        };
        let key = SnapshotKey {
            shard,
            generation,
            seq: state.shard_seq,
            ptr,
        };
        let taken_at = self.inner.clock.now();
        snapshot::write(store, &key, &state.to_snapshot(taken_at))
            .await
            .map_err(io)?;

        // 5. Serve.
        state.phase = ShardPhase::Serving;
        state.snapshot_seq = state.shard_seq;
        state.snapshot_key = Some(key);
        state.snapshot_at = Some(taken_at);
        state.records_since_snapshot = 0;
        state.dirty_since_lsn = None;
        {
            let mut st = self.inner.shards[shard.index()].lock();
            if st.phase != ShardPhase::Loading {
                return Err(ClaimRefused::Invariant(
                    "shard changed phase while loading".into(),
                ));
            }
            *st = state;
        }
        self.inner.stranded.lock().remove(&shard);
        let now = tokio::time::Instant::now();
        self.inner.moved_at.lock().insert(shard, now);
        let grace = Duration::from_millis(self.inner.cfg.cluster.reconcile_grace_ms);
        if !grace.is_zero() {
            self.inner.hold_until.lock().insert(shard, now + grace);
        }
        self.rebuild_shard_indexes(shard);
        Ok(())
    }

    // ───────────────────────── release ─────────────────────────

    /// Hand `shards` back (at most `limit`). Returns how many were released.
    pub async fn release_shards(&self, shards: Vec<ShardId>, limit: usize) -> usize {
        let _moves = self.inner.moves.lock().await;
        // Shards release independently; a drain hands over every shard, so do it in
        // parallel or it outlasts the lease timeout on a slow bucket.
        let concurrency = self.inner.cfg.cluster.claim_concurrency.max(1);
        let results: Vec<(ShardId, Result<bool, ServerError>)> =
            futures::stream::iter(shards.into_iter().take(limit))
                .map(|s| async move { (s, self.release_one(s).await) })
                .buffer_unordered(concurrency)
                .collect()
                .await;
        let mut released = 0;
        for (shard, r) in results {
            match r {
                Ok(true) => {
                    released += 1;
                    valka_core::metrics::record_shard_release("released");
                }
                Ok(false) => {}
                Err(e) => {
                    valka_core::metrics::record_shard_release("failed");
                    warn!(shard = %shard, error = %e, "release failed")
                }
            }
        }
        if released > 0 {
            info!(released, "released shards");
        }
        released
    }

    async fn release_one(&self, shard: ShardId) -> Result<bool, ServerError> {
        let generation = {
            let mut st = self.inner.shards[shard.index()].lock();
            if st.phase != ShardPhase::Serving {
                return Ok(false);
            }
            st.phase = ShardPhase::Releasing;
            st.generation
        };
        // Coalesced heartbeats for this shard go into the WAL before the final snapshot.
        self.flush_lease_records_for(Some(shard));
        let back_to_serving = |e: ServerError| {
            let mut st = self.inner.shards[shard.index()].lock();
            if st.phase == ShardPhase::Releasing {
                st.phase = ShardPhase::Serving;
            }
            e
        };
        self.inner
            .writer
            .sync()
            .await
            .map_err(|e| back_to_serving(e.into()))?;
        let (key, snap) = {
            let st = self.inner.shards[shard.index()].lock();
            let key = SnapshotKey {
                shard,
                generation,
                seq: st.shard_seq,
                ptr: LogPtr::Final,
            };
            (key, st.to_snapshot(self.inner.clock.now()))
        };
        snapshot::write(&self.inner.store, &key, &snap)
            .await
            .map_err(|e| back_to_serving(e.into()))?;
        let current = owners::get(&self.inner.store, shard)
            .await
            .map_err(|e| back_to_serving(e.into()))?;
        let mine = current.as_ref().is_some_and(|v| {
            v.record.node == self.node_id()
                && v.record.inc == self.incarnation()
                && v.record.generation == generation
                && v.record.state == OwnerState::Owned
        });
        let cas = if mine {
            owners::cas(
                &self.inner.store,
                shard,
                current.as_ref(),
                &OwnerRecord {
                    node: self.node_id().to_string(),
                    inc: self.incarnation(),
                    generation,
                    state: OwnerState::Released,
                },
            )
            .await
        } else {
            Ok(false)
        };
        // Whatever happened, the shard is no longer ours to serve from this state: released,
        // taken over after this node was declared dead, or (CAS failed) unknown. A failed CAS
        // may have landed with its answer lost, or land later, and another node may already
        // be claiming the shard: serving it again here would be two writers. Found by the
        // randomized simulation (a lost CAS answer put the shard back to Serving while the
        // bucket said released; the new owner never saw the writes that followed).
        {
            let mut st = self.inner.shards[shard.index()].lock();
            *st = ShardState::new(shard);
        }
        let ok = match cas {
            Ok(ok) => ok,
            Err(e) => {
                // Settle it through a claim: if the record still names this incarnation the
                // claim takes the next generation from the final snapshot just written
                // (which also fences the late CAS); if it was released, whoever placement
                // picks claims it.
                self.inner.stranded.lock().insert(shard);
                warn!(shard = %shard, error = %e, "release outcome unknown; the shard will be claimed again");
                return Err(e.into());
            }
        };
        self.inner
            .moved_at
            .lock()
            .insert(shard, tokio::time::Instant::now());
        self.inner.hold_until.lock().remove(&shard);
        if !ok {
            error!(shard = %shard, owner = ?current.map(|v| v.record), "owner record is not ours at release; this node was taken over");
        }
        let _ = snapshot::prune(
            &self.inner.store,
            shard,
            self.inner.cfg.wal.snapshots_to_keep,
        )
        .await;
        Ok(ok)
    }

    // ───────────────────────── split ─────────────────────────

    /// Split a dead incarnation's log into per-shard snapshots and mark it split.
    /// Idempotent: running it again (or concurrently elsewhere) writes the same snapshots.
    pub async fn split_incarnation(
        &self,
        node: &str,
        inc: u32,
        trunc: u64,
    ) -> Result<(), ServerError> {
        let r = async {
            let Some(log) = self.split_read(node, inc, trunc).await? else {
                return Ok(());
            };
            self.split_commit(node, inc, log).await
        }
        .await;
        if r.is_err() {
            valka_core::metrics::record_split_failed();
        }
        r
    }

    /// Phase 1: read the log from the watermark to its sealed end. `None` when the
    /// incarnation is already split.
    pub(crate) async fn split_read(
        &self,
        node: &str,
        inc: u32,
        trunc: u64,
    ) -> Result<Option<split::SealedLog>, ServerError> {
        let store = &self.inner.store;
        if split::marker_exists(store, node, inc).await? {
            return Ok(None);
        }
        match split::read_sealed(store, node, inc, trunc).await {
            Ok(l) => Ok(Some(l)),
            // Segments at or above the watermark vanish only once a split has finished.
            Err(_) if split::marker_exists(store, node, inc).await? => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Phase 2: write the split snapshots and the marker. Everything this builds on is
    /// read *after* the log (PHASE2 §8.3 steps 4–5): the marker re-check, the owner
    /// records, the base snapshots.
    pub(crate) async fn split_commit(
        &self,
        node: &str,
        inc: u32,
        log: split::SealedLog,
    ) -> Result<(), ServerError> {
        let store = &self.inner.store;
        let started = std::time::Instant::now();
        // A split that finished while this one was reading may have deleted the log, seal
        // included, and a late PUT may have refilled the sealed position: this read is
        // then not the log the finished split read. Write nothing.
        if split::marker_exists(store, node, inc).await? {
            return Ok(());
        }
        let total = log.records.len();
        let mut by_shard: BTreeMap<ShardId, Vec<Envelope>> = BTreeMap::new();
        for (_, env) in log.records {
            by_shard.entry(env.shard).or_default().push(env);
        }
        // Rebuild only the shards whose owner record names this incarnation, at that
        // record's generation. Older generations in the log ended with a final snapshot.
        let written: Vec<Result<bool, ServerError>> = futures::stream::iter(by_shard)
            .map(|(shard, records)| async move {
                if shard.0 as u32 >= self.inner.num_shards {
                    warn!(shard = %shard, "split: records for a shard outside the cluster");
                    return Ok(false);
                }
                let Some(owner) = owners::get(store, shard).await? else {
                    return Ok(false);
                };
                if owner.record.node != node || owner.record.inc != inc {
                    return Ok(false);
                }
                self.split_shard(shard, owner.record.generation, records)
                    .await
            })
            .buffer_unordered(32)
            .collect()
            .await;
        let mut snapshots_written = 0;
        for w in written {
            if w? {
                snapshots_written += 1;
            }
        }
        split::write_marker(
            store,
            &SplitMarker {
                node: node.to_string(),
                inc,
                split_by: self.node_id().to_string(),
                at: chrono::Utc::now(),
                end: log.end,
                records: total,
                snapshots_written,
            },
        )
        .await?;
        valka_core::metrics::record_split(total, started.elapsed().as_secs_f64());
        info!(
            node,
            inc,
            records = total,
            segments = log.segments_read,
            snapshots = snapshots_written,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "split dead incarnation"
        );
        // Claims refused while this log was unsplit can go through now.
        self.clear_claim_backoff();
        if let Err(e) = split::delete_incarnation_log(store, node, inc).await {
            warn!(node, inc, error = %e, "deleting split log failed; the collector retries");
        }
        Ok(())
    }

    /// Rebuild one shard at `generation` from its newest snapshot plus `records`.
    /// The applied records must continue the base's `shard_seq` without a gap; a gap is
    /// a bug, never data to serve, so the split fails instead of writing it.
    async fn split_shard(
        &self,
        shard: ShardId,
        generation: u64,
        records: Vec<Envelope>,
    ) -> Result<bool, ServerError> {
        let store = &self.inner.store;
        let listed = snapshot::list(store, shard).await?;
        if listed.iter().any(|k| k.generation > generation) {
            // The shard moved on after this log was split once already.
            return Ok(false);
        }
        let base = snapshot::load_newest_of::<ShardSnapshot>(store, &listed, |k| {
            k.generation <= generation
        })
        .await?;
        let mut st = match base {
            Some((key, snap)) => ShardState::from_snapshot(snap, Some(key)),
            None => ShardState::new(shard),
        };
        st.generation = generation;
        let mut applied = 0usize;
        let mut skipped_gen = HashSet::new();
        for mut env in records {
            if env.generation != generation {
                skipped_gen.insert(env.generation);
                continue;
            }
            if env.shard_seq <= st.shard_seq {
                continue;
            }
            if env.shard_seq != st.shard_seq + 1 {
                return Err(ServerError::Internal(format!(
                    "split of shard {shard} gen {generation}: shard_seq jumps from {} to {}",
                    st.shard_seq, env.shard_seq
                )));
            }
            st.apply(&mut env);
            applied += 1;
        }
        if !skipped_gen.is_empty() {
            debug!(shard = %shard, generation, ?skipped_gen, "split skipped records of older generations");
        }
        if applied == 0 {
            return Ok(false);
        }
        let key = SnapshotKey {
            shard,
            generation,
            seq: st.shard_seq,
            ptr: LogPtr::Split,
        };
        snapshot::write(store, &key, &st.to_snapshot(chrono::Utc::now())).await?;
        Ok(true)
    }

    /// The owner record of a shard (admin, diagnostics).
    pub async fn owner_record(&self, shard: ShardId) -> Result<Option<Versioned>, ServerError> {
        Ok(owners::get(&self.inner.store, shard).await?)
    }
}
