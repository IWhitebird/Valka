//! Regression tests for the protocol bugs the TLA+ model found (spec/tla/README.md,
//! "What the model found"), plus the per-shard log-order invariant replay relies on.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use valka_core::{ServerError, ShardId};
use valka_wal::lsn::keys;
use valka_wal::owners::{self, OwnerRecord, OwnerState};
use valka_wal::{Envelope, Lsn, Store, WalRecord, lease, reader, segment, snapshot, split};

use crate::clock::TokioClock;
use crate::engine::{CreateTask, Engine, EngineConfig};

fn cfg(node: &str) -> EngineConfig {
    let mut c = EngineConfig::for_tests(node);
    c.cluster.num_shards = 16;
    c
}

async fn open(store: &Store, node: &str) -> Engine {
    Engine::open_with(
        store.clone(),
        cfg(node),
        TokioClock::new(),
        Arc::new(crate::sink::NoopSink),
    )
    .await
    .unwrap()
}

fn create(queue: &str) -> CreateTask {
    CreateTask {
        queue_name: queue.into(),
        task_name: "t".into(),
        input: None,
        priority: 0,
        max_retries: 3,
        timeout_seconds: 300,
        idempotency_key: None,
        metadata: serde_json::json!({}),
        scheduled_at: None,
    }
}

fn snapshot_count(all: &std::collections::BTreeMap<ShardId, Vec<snapshot::SnapshotKey>>) -> usize {
    all.values().map(Vec::len).sum()
}

/// Model bug 7 (`LeaseGC`): leases are never deleted, however long dead.
#[tokio::test(start_paused = true)]
async fn garbage_collection_never_deletes_leases() {
    let store = Store::memory();
    let a = open(&store, "a").await;
    a.drain().await.unwrap();
    let b = open(&store, "b").await;
    // Jump the clock (a sleep would run every timer tick of three hours).
    tokio::time::advance(Duration::from_secs(3 * 3600)).await;
    b.refresh_membership().await;
    b.gc_round().await.unwrap();
    let l = lease::get(&store, "a").await.unwrap().expect("lease kept");
    assert_eq!(l.record.state, lease::LeaseState::Dead);
}

/// Model bug 4 (`NoMarkerRecheck`): a splitter whose read raced a finished split (log
/// collected, sealed position refilled by a late PUT) must write nothing.
#[tokio::test(start_paused = true)]
async fn slow_splitter_rechecks_the_marker_before_writing() {
    let store = Store::memory();
    let a = open(&store, "a").await;
    let id = a.create_task(create("q")).await.unwrap().id;
    let shard = a.shard_of(&id).unwrap();
    a.sync().await.unwrap();
    a.halt();
    let trunc = lease::get(&store, "a").await.unwrap().unwrap().record.trunc;

    // A second process of "a" fences the first and splits its log (marker written, log
    // deleted, seal included).
    let a2 = open(&store, "a").await;
    assert!(split::marker_exists(&store, "a", 1).await.unwrap());
    let before = snapshot::list_all(&store).await.unwrap();

    // With the log collected, a very late PUT of the dead writer lands at a freed
    // position, carrying a record the acknowledged history never had.
    let late_lsn = Lsn::new(1, trunc);
    let mut rogue = Envelope::new(
        shard,
        WalRecord::TaskDeleted {
            task_id: id.clone(),
        },
    );
    rogue.generation = 1;
    rogue.shard_seq = 2;
    store
        .put(
            &keys::segment("a", late_lsn),
            segment::encode(late_lsn, "a", &[rogue], true).unwrap(),
        )
        .await
        .unwrap();

    // A slow splitter that read now (bypassing its first marker check)...
    let log = split::read_sealed(&store, "a", 1, trunc).await.unwrap();
    assert!(!log.records.is_empty());
    // ...re-checks the marker before writing, and writes nothing.
    a2.split_commit("a", 1, log).await.unwrap();
    assert_eq!(
        snapshot_count(&snapshot::list_all(&store).await.unwrap()),
        snapshot_count(&before)
    );
    assert!(a2.get_task(&id).is_some(), "the task was never deleted");
}

/// Model bug 3: a split rebuilds only shards whose owner record names the dead
/// incarnation; older generations in the log ended with a final snapshot.
#[tokio::test(start_paused = true)]
async fn split_rebuilds_only_shards_the_dead_incarnation_owns() {
    let store = Store::memory();
    let a = open(&store, "a").await;
    let id = a.create_task(create("q")).await.unwrap().id;
    let shard = a.shard_of(&id).unwrap();
    a.sync().await.unwrap();
    a.halt();
    // Pretend the shard had moved on: its owner record names another incarnation.
    let cur = owners::get(&store, shard).await.unwrap().unwrap();
    assert!(
        owners::cas(
            &store,
            shard,
            Some(&cur),
            &OwnerRecord {
                node: "elsewhere".into(),
                inc: 1,
                generation: cur.record.generation + 1,
                state: OwnerState::Released,
            },
        )
        .await
        .unwrap()
    );
    let before = snapshot::list(&store, shard).await.unwrap();
    let a2 = open(&store, "a").await; // splits incarnation 1
    assert!(split::marker_exists(&store, "a", 1).await.unwrap());
    let after = snapshot::list(&store, shard).await.unwrap();
    assert!(
        !after.iter().any(|k| k.ptr == valka_wal::LogPtr::Split),
        "no split snapshot for a shard the dead incarnation no longer owned: {after:?}"
    );
    assert!(after.len() >= before.len());
    drop(a2);
}

/// A gap in a shard's `shard_seq` inside a dead log is a bug, never data to serve: the
/// split fails and writes no marker, so the shard stays unclaimable and alerts.
#[tokio::test(start_paused = true)]
async fn split_refuses_a_shard_seq_gap() {
    let store = Store::memory();
    let a = open(&store, "a").await;
    let id = a.create_task(create("q")).await.unwrap().id;
    let shard = a.shard_of(&id).unwrap();
    a.sync().await.unwrap();
    a.halt();
    let l = lease::get(&store, "a").await.unwrap().unwrap();
    // Append a record that skips shard_seq 2 at the next free position.
    let segs = reader::list_segments(&store, "a", 1).await.unwrap();
    let next = Lsn::new(1, segs.last().unwrap().0.seq + 1);
    let mut skip = Envelope::new(
        shard,
        WalRecord::TaskPromoted {
            task_id: id.clone(),
        },
    );
    skip.generation = owners::get(&store, shard)
        .await
        .unwrap()
        .unwrap()
        .record
        .generation;
    skip.shard_seq = 3;
    store
        .put(
            &keys::segment("a", next),
            segment::encode(next, "a", &[skip], true).unwrap(),
        )
        .await
        .unwrap();
    let b = open(&store, "b").await;
    let err = b
        .split_incarnation("a", 1, l.record.trunc)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ServerError::Internal(ref m) if m.contains("jumps")),
        "{err}"
    );
    assert!(!split::marker_exists(&store, "a", 1).await.unwrap());
}

/// Replay and splits apply a shard's records in log order and skip any at or below the
/// current `shard_seq`, so the log must hold each shard's records in `shard_seq` order,
/// also when coalesced heartbeat records race ordinary writes on other threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn log_holds_each_shards_records_in_order_under_concurrency() {
    let store = Store::memory();
    // One partition: every task of the queue shares one shard, maximising contention.
    valka_wal::queues::set_partitions(&store, "hot", 1, 16)
        .await
        .unwrap();
    let e = open(&store, "n").await;
    let running: Vec<String> = {
        let mut v = Vec::new();
        for _ in 0..8 {
            let t = e.create_task(create("hot")).await.unwrap();
            e.dispatch(&t.id, "w").unwrap();
            v.push(t.id);
        }
        v
    };
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hb = {
        let (e, ids, stop) = (e.clone(), running.clone(), stop.clone());
        tokio::spawn(async move {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                e.heartbeat(&ids);
                e.flush_lease_records();
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
    };
    let mut writers = Vec::new();
    for _ in 0..3 {
        let e = e.clone();
        writers.push(tokio::spawn(async move {
            for _ in 0..40 {
                e.create_task(create("hot")).await.unwrap();
            }
        }));
    }
    for w in writers {
        w.await.unwrap();
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    hb.await.unwrap();
    e.sync().await.unwrap();

    let mut last: HashMap<ShardId, u64> = HashMap::new();
    let mut leases = 0;
    for (_, env) in reader::read_all(&store, "n", e.incarnation())
        .await
        .unwrap()
    {
        if env.record.kind() == "lease_extended" {
            leases += 1;
        }
        let prev = last.insert(env.shard, env.shard_seq).unwrap_or(0);
        assert_eq!(
            env.shard_seq,
            prev + 1,
            "shard {} record {} out of order after {prev}",
            env.shard,
            env.record.kind()
        );
    }
    assert!(leases > 0, "the race was exercised");
}

/// A node refuses to join a cluster that already runs a newer protocol.
#[tokio::test(start_paused = true)]
async fn a_node_refuses_to_join_a_newer_protocol() {
    let store = Store::memory();
    let (newer, _) = lease::LeaseKeeper::start(
        store.clone(),
        "newer",
        Default::default(),
        valka_wal::LeaseConfig::from_core(&cfg("newer").cluster),
    )
    .await
    .unwrap();
    // Pretend it speaks a future protocol.
    newer
        .update(|r| r.protocol = valka_wal::lease::PROTOCOL + 1)
        .await
        .unwrap();
    let err = Engine::open_with(
        store.clone(),
        cfg("older"),
        TokioClock::new(),
        Arc::new(crate::sink::NoopSink),
    )
    .await
    .err()
    .expect("must refuse");
    assert!(err.to_string().contains("protocol"), "{err}");
    // Once the newer node is gone, the older one may start.
    newer.set_state(lease::LeaseState::Dead).await.unwrap();
    open(&store, "older").await;
}
