//! Several engines on one bucket: placement, takeover, fencing, drain (docs/wal/PHASE2.md
//! §13). Each engine sees the shared in-memory bucket through its own fault injector, and
//! tokio time is paused, so leases, timeouts and placement rounds run deterministically.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use valka_core::{ServerError, ShardId, TaskStatus};
use valka_wal::fault::faulty_over;
use valka_wal::owners::{self, OwnerRecord, OwnerState};
use valka_wal::{FaultConfig, Store, lease};

use crate::clock::TokioClock;
use crate::engine::{CreateTask, Engine, EngineConfig};

const SHARDS: u32 = 64;

fn cfg(node: &str) -> EngineConfig {
    let mut c = EngineConfig::for_tests(node);
    c.cluster.num_shards = SHARDS;
    c.cluster.lease_interval_ms = 500;
    c.cluster.lease_timeout_ms = 3_000;
    c.cluster.lease_safety_margin_ms = 1_000;
    c.cluster.membership_interval_ms = 300;
    c.cluster.placement_interval_ms = 200;
    c.cluster.split_timeout_ms = 5_000;
    c
}

struct Node {
    engine: Engine,
    faults: Arc<FaultConfig>,
}

type Backing = Arc<dyn object_store::ObjectStore>;

fn bucket() -> Backing {
    Arc::new(object_store::memory::InMemory::new())
}

async fn start(backing: &Backing, node: &str) -> Node {
    start_with(backing, cfg(node)).await
}

async fn start_with(backing: &Backing, cfg: EngineConfig) -> Node {
    let (store, faults) = faulty_over(backing.clone(), cfg.node_id.len() as u64);
    let engine = Engine::open_with(
        store,
        cfg,
        TokioClock::new(),
        Arc::new(crate::sink::NoopSink),
    )
    .await
    .unwrap();
    Node { engine, faults }
}

fn plain(backing: &Backing) -> Store {
    Store::wrap(backing.clone(), "", true, "memory")
}

fn create(queue: &str) -> CreateTask {
    CreateTask {
        queue_name: queue.into(),
        task_name: "job".into(),
        input: None,
        priority: 0,
        max_retries: 3,
        timeout_seconds: 300,
        idempotency_key: None,
        metadata: serde_json::json!({}),
        scheduled_at: None,
    }
}

/// Create a task on whichever node owns the shard its id lands in (routing is the API
/// layer's job; here we just try until a node accepts).
async fn create_anywhere(nodes: &[&Engine]) -> String {
    for _ in 0..200 {
        for e in nodes {
            match e.create_task(create("q")).await {
                Ok(v) => return v.id,
                Err(ServerError::NotOwner(_)) => continue,
                Err(e) => panic!("create failed: {e}"),
            }
        }
    }
    panic!("no node accepted a create");
}

fn owner_of<'a>(nodes: &[&'a Engine], id: &str) -> Option<&'a Engine> {
    nodes
        .iter()
        .copied()
        .find(|e| e.shard_of(id).is_some_and(|s| e.owns(s)))
}

/// Every shard served by exactly one of `nodes`.
fn assert_partition(nodes: &[&Engine]) {
    let mut seen: HashMap<ShardId, String> = HashMap::new();
    for e in nodes {
        for s in e.owned_shards() {
            if let Some(other) = seen.insert(s, e.node_id().to_string()) {
                panic!("shard {s} served by both {other} and {}", e.node_id());
            }
        }
    }
    assert_eq!(seen.len(), SHARDS as usize, "every shard is served");
}

async fn wait_until(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    for _ in 0..secs * 10 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test(start_paused = true)]
async fn two_nodes_split_the_shards_by_rendezvous() {
    let b = bucket();
    let a = start(&b, "a").await;
    assert_eq!(a.engine.owned_shards().len(), SHARDS as usize);
    let n2 = start(&b, "b").await;
    wait_until("both nodes serve their rendezvous share", 20, || {
        let eligible = vec!["a".to_string(), "b".to_string()];
        ShardId::all(SHARDS).all(|s| {
            let want = a.engine.desired_owner(s, &eligible).unwrap();
            let e = if want == "a" { &a.engine } else { &n2.engine };
            e.owns(s)
        })
    })
    .await;
    assert_partition(&[&a.engine, &n2.engine]);
    let (na, nb) = (
        a.engine.owned_shards().len(),
        n2.engine.owned_shards().len(),
    );
    assert!(na > 10 && nb > 10, "a={na} b={nb}");

    // Moved shards went through release (final snapshot) and claim (gen 2).
    let moved = n2.engine.owned_shards()[0];
    let rec = owners::get(&plain(&b), moved)
        .await
        .unwrap()
        .unwrap()
        .record;
    assert_eq!((rec.node.as_str(), rec.generation), ("b", 2));
}

#[tokio::test(start_paused = true)]
async fn killed_node_is_tombstoned_split_and_taken_over() {
    let b = bucket();
    let a = start(&b, "a").await;
    let n2 = start(&b, "b").await;
    wait_until("converged", 20, || {
        a.engine.owned_shards().len() + n2.engine.owned_shards().len() == SHARDS as usize
            && !n2.engine.owned_shards().is_empty()
            && !a.engine.owned_shards().is_empty()
    })
    .await;
    let nodes = [&a.engine, &n2.engine];
    let mut expected: HashMap<String, TaskStatus> = HashMap::new();
    for i in 0..60 {
        let id = create_anywhere(&nodes).await;
        let mut status = TaskStatus::Pending;
        if i % 3 == 0 {
            let e = owner_of(&nodes, &id).unwrap();
            let d = e.dispatch(&id, "w").unwrap();
            e.complete_run(&id, &d.run_id, None).await.unwrap();
            status = TaskStatus::Completed;
        }
        expected.insert(id, status);
    }
    assert!(
        expected
            .keys()
            .any(|id| owner_of(&nodes, id).is_some_and(|e| e.node_id() == "a")),
        "some acked work lives on the node we kill"
    );

    // kill -9: no more renewals, no flush, nothing.
    a.engine.halt();
    a.faults.set_puts_down(true);
    wait_until("b serves every shard", 30, || {
        n2.engine.owned_shards().len() == SHARDS as usize
    })
    .await;
    for (id, status) in &expected {
        let t = n2
            .engine
            .get_task(id)
            .unwrap_or_else(|| panic!("acked task {id} lost"));
        assert_eq!(t.status, *status, "task {id}");
    }
    let dead = lease::get(&plain(&b), "a").await.unwrap().unwrap().record;
    assert_eq!(dead.state, lease::LeaseState::Dead);
    assert_eq!(dead.dead_by.as_deref(), Some("b"));
    assert!(
        valka_wal::split::marker_exists(&plain(&b), "a", 1)
            .await
            .unwrap()
    );
}

#[tokio::test(start_paused = true)]
async fn frozen_node_is_fenced_when_it_wakes_up() {
    let b = bucket();
    let a = start(&b, "a").await;
    let n2 = start(&b, "b").await;
    wait_until("converged", 20, || {
        !n2.engine.owned_shards().is_empty() && !a.engine.owned_shards().is_empty()
    })
    .await;
    let nodes = [&a.engine, &n2.engine];
    let mut on_a = Vec::new();
    while on_a.len() < 5 {
        let id = create_anywhere(&nodes).await;
        if owner_of(&nodes, &id).unwrap().node_id() == "a" {
            on_a.push(id);
        }
    }

    // A freezes (SIGSTOP, GC pause, VM freeze) in the middle of a write.
    a.faults.freeze(true);
    let shard = a.engine.owned_shards()[0];
    let in_flight = {
        let e = a.engine.clone();
        tokio::spawn(async move {
            loop {
                let r = e.create_task(create("q")).await;
                match r {
                    Err(ServerError::NotOwner(_)) => continue,
                    other => return other,
                }
            }
        })
    };
    wait_until("b took every shard", 30, || {
        n2.engine.owned_shards().len() == SHARDS as usize
    })
    .await;
    let _ = shard;

    // A wakes up believing it still owns its shards. Nothing it does now may be acked.
    a.faults.freeze(false);
    let r = in_flight.await.unwrap();
    assert!(
        r.is_err(),
        "a write from a fenced node must not be acked: {r:?}"
    );
    wait_until("a knows it is fenced", 10, || a.engine.poisoned().is_some()).await;
    assert!(a.engine.create_task(create("q")).await.is_err());
    for id in &on_a {
        assert!(n2.engine.get_task(id).is_some(), "acked task {id} survived");
    }
}

#[tokio::test(start_paused = true)]
async fn node_cut_off_from_the_bucket_stops_serving_before_takeover() {
    let b = bucket();
    let a = start(&b, "a").await;
    let n2 = start(&b, "b").await;
    wait_until("converged", 20, || !n2.engine.owned_shards().is_empty()).await;
    // A can no longer write to the bucket: renewals fail.
    a.faults.set_puts_down(true);
    // timeout 3 s - margin 1 s: A goes quiet after 2 s, B may take over after 3 s.
    tokio::time::sleep(Duration::from_millis(2_300)).await;
    assert!(!a.engine.is_serving(), "self-fenced");
    assert!(
        a.engine.owned_shards().iter().any(|s| !n2.engine.owns(*s)),
        "and nobody has taken over yet"
    );
    let r = a.engine.create_task(create("q")).await;
    assert!(
        matches!(
            r,
            Err(ServerError::Unavailable(_)) | Err(ServerError::NotOwner(_))
        ),
        "{r:?}"
    );
    wait_until("b takes over", 30, || {
        n2.engine.owned_shards().len() == SHARDS as usize
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn drain_hands_shards_over_with_nothing_to_replay() {
    let b = bucket();
    let a = start(&b, "a").await;
    let n2 = start(&b, "b").await;
    wait_until("converged", 20, || !n2.engine.owned_shards().is_empty()).await;
    let nodes = [&a.engine, &n2.engine];
    let mut ids = Vec::new();
    for _ in 0..40 {
        ids.push(create_anywhere(&nodes).await);
    }
    a.engine.drain().await.unwrap();
    assert!(a.engine.owned_shards().is_empty());
    let l = lease::get(&plain(&b), "a").await.unwrap().unwrap().record;
    assert_eq!(l.state, lease::LeaseState::Dead);
    assert_eq!(
        l.dead_by.as_deref(),
        Some("a"),
        "a clean exit, not a tombstone"
    );
    assert!(
        valka_wal::reader::list_segments(&plain(&b), "a", 1)
            .await
            .unwrap()
            .is_empty(),
        "a drained node leaves no log behind"
    );
    wait_until("b serves everything", 30, || {
        n2.engine.owned_shards().len() == SHARDS as usize
    })
    .await;
    for id in &ids {
        assert_eq!(
            n2.engine.get_task(id).unwrap().status,
            TaskStatus::Pending,
            "task {id}"
        );
    }
    // Released shards were claimed from their final snapshots.
    let released = valka_wal::snapshot::list_all(&plain(&b)).await.unwrap();
    assert!(
        released
            .values()
            .flatten()
            .any(|k| k.ptr == valka_wal::LogPtr::Final)
    );
}

#[tokio::test(start_paused = true)]
async fn a_drain_slower_than_the_lease_timeout_keeps_its_lease() {
    // Releasing every shard on a slow bucket takes far longer than the lease timeout. The
    // draining node must keep renewing its lease meanwhile, or a guardian tombstones it
    // halfway and the rest of its shards go through a split instead of a handover.
    let b = bucket();
    let mut slow = cfg("a");
    slow.cluster.claim_concurrency = 1;
    let a = start_with(&b, slow).await;
    let n2 = start(&b, "b").await;
    wait_until("converged", 20, || !n2.engine.owned_shards().is_empty()).await;
    let nodes = [&a.engine, &n2.engine];
    let mut ids = Vec::new();
    for _ in 0..20 {
        ids.push(create_anywhere(&nodes).await);
    }
    let owned = a.engine.owned_shards().len();
    a.faults
        .latency_ms
        .store(100, std::sync::atomic::Ordering::Relaxed);
    let started = tokio::time::Instant::now();
    a.engine.drain().await.unwrap();
    let took = started.elapsed();
    assert!(
        took > Duration::from_millis(cfg("a").cluster.lease_timeout_ms),
        "the drain of {owned} shards took {took:?}: not slow enough to test anything"
    );
    let l = lease::get(&plain(&b), "a").await.unwrap().unwrap().record;
    assert_eq!(
        l.dead_by.as_deref(),
        Some("a"),
        "a clean exit, not a tombstone"
    );
    wait_until("b serves everything", 30, || {
        n2.engine.owned_shards().len() == SHARDS as usize
    })
    .await;
    for id in &ids {
        assert_eq!(n2.engine.get_task(id).unwrap().status, TaskStatus::Pending);
    }
    // Every shard a owned was handed over with a final snapshot, none split.
    let snaps = valka_wal::snapshot::list_all(&plain(&b)).await.unwrap();
    assert!(
        !snaps
            .values()
            .flatten()
            .any(|k| k.ptr == valka_wal::LogPtr::Split),
        "no shard of the draining node was split"
    );
}

#[tokio::test(start_paused = true)]
async fn a_release_whose_cas_answer_is_lost_never_leaves_two_writers() {
    // Found by the randomized simulation: the owner's CAS to "released" landed but its
    // answer was lost; the owner took the error as "not released" and served the shard
    // again while another node claimed it. Writes acknowledged by the old owner after
    // that were never seen by the new one.
    let b = bucket();
    let a = start(&b, "a").await;
    let n2 = start(&b, "b").await;
    wait_until("converged", 20, || !n2.engine.owned_shards().is_empty()).await;
    let nodes = [&a.engine, &n2.engine];
    let id = loop {
        let id = create_anywhere(&nodes).await;
        if owner_of(&nodes, &id).is_some_and(|e| e.node_id() == "a") {
            break id;
        }
    };
    let shard = a.engine.shard_of(&id).unwrap();

    a.faults.lose_next_put_acks("owners/", 1);
    let released = a.engine.release_shards(vec![shard], 1).await;
    assert_eq!(released, 0, "the release reported an error");
    assert!(
        !a.engine.owns(shard),
        "a release with an unknown outcome must not go back to serving"
    );

    // The shard is claimed again (by a, settling its stranded release, or by b) and served
    // by exactly one node, with the task it held.
    wait_until("the shard is served again", 30, || {
        nodes.iter().filter(|e| e.owns(shard)).count() == 1
    })
    .await;
    let owner = owner_of(&nodes, &id).unwrap();
    assert_eq!(owner.get_task(&id).unwrap().status, TaskStatus::Pending);
    // A write acknowledged now is seen after the next move too.
    let more = owner
        .create_task_at(&valka_core::new_id_in_shard(shard).to_string(), create("q"))
        .await
        .unwrap();
    owner.sync().await.unwrap();
    let other = if owner.node_id() == "a" {
        &n2.engine
    } else {
        &a.engine
    };
    assert!(!other.owns(shard));
    let released = owner.release_shards(vec![shard], 1).await;
    assert_eq!(released, 1);
    wait_until("handed over", 30, || {
        nodes.iter().filter(|e| e.owns(shard)).count() == 1
    })
    .await;
    let owner = owner_of(&nodes, &id).unwrap();
    assert!(owner.get_task(&id).is_some());
    assert!(
        owner.get_task(&more.id).is_some(),
        "the acknowledged write survived the move"
    );
    assert_partition(&nodes);
}

#[tokio::test(start_paused = true)]
async fn a_claim_that_crashed_before_serving_is_skipped_over() {
    // A claimer CAS'd the owner record (gen 2) and died before writing its acquisition
    // snapshot. Its log has nothing for the shard. The next claim takes gen 3 and starts
    // from the gen 1 snapshot.
    let b = bucket();
    let store = plain(&b);
    let a = start(&b, "a").await;
    let id = a.engine.create_task(create("q")).await.unwrap().id;
    let shard = a.engine.shard_of(&id).unwrap();
    a.engine.drain().await.unwrap();
    // Ghost claimer: dead lease, split marker, owner record at gen 2, no snapshot at 2.
    let (ghost, _) = lease::LeaseKeeper::start(
        store.clone(),
        "ghost",
        Default::default(),
        valka_wal::LeaseConfig::from_core(&cfg("ghost").cluster),
    )
    .await
    .unwrap();
    let cur = owners::get(&store, shard).await.unwrap().unwrap();
    assert!(
        owners::cas(
            &store,
            shard,
            Some(&cur),
            &OwnerRecord {
                node: "ghost".into(),
                inc: ghost.inc(),
                generation: cur.record.generation + 1,
                state: OwnerState::Owned,
            },
        )
        .await
        .unwrap()
    );
    ghost.set_state(lease::LeaseState::Dead).await.unwrap();
    valka_wal::split::read_sealed(&store, "ghost", ghost.inc(), 1)
        .await
        .unwrap();
    valka_wal::split::write_marker(
        &store,
        &valka_wal::split::SplitMarker {
            node: "ghost".into(),
            inc: ghost.inc(),
            split_by: "test".into(),
            at: chrono::Utc::now(),
            end: 1,
            records: 0,
            snapshots_written: 0,
        },
    )
    .await
    .unwrap();

    let c = start(&b, "c").await;
    assert!(c.engine.owns(shard));
    assert_eq!(c.engine.get_task(&id).unwrap().status, TaskStatus::Pending);
    let rec = owners::get(&store, shard).await.unwrap().unwrap().record;
    assert_eq!(rec.node, "c");
    assert_eq!(rec.generation, cur.record.generation + 2);
}

#[tokio::test(start_paused = true)]
async fn restarted_node_splits_its_own_log_and_reclaims() {
    let b = bucket();
    let a = start(&b, "a").await;
    let mut ids = BTreeSet::new();
    for _ in 0..30 {
        ids.insert(a.engine.create_task(create("q")).await.unwrap().id);
    }
    a.engine.sync().await.unwrap();
    a.engine.halt();
    drop(a);
    let a2 = start(&b, "a").await;
    assert_eq!(a2.engine.incarnation(), 2);
    assert_eq!(a2.engine.owned_shards().len(), SHARDS as usize);
    for id in &ids {
        assert!(a2.engine.get_task(id).is_some());
    }
    let l = lease::get(&plain(&b), "a").await.unwrap().unwrap().record;
    assert!(
        l.unsplit.is_empty(),
        "the old incarnation was split and forgotten"
    );
    assert!(
        valka_wal::split::marker_exists(&plain(&b), "a", 1)
            .await
            .unwrap()
    );
}

#[tokio::test(start_paused = true)]
async fn garbage_collector_removes_split_logs_and_old_snapshots() {
    let b = bucket();
    let store = plain(&b);
    let a = start(&b, "a").await;
    for _ in 0..10 {
        a.engine.create_task(create("q")).await.unwrap();
    }
    a.engine.snapshot_now().await;
    a.engine.snapshot_now().await;
    a.engine.halt();
    drop(a);
    // Leave a stray segment of the split incarnation behind, as if the splitter's
    // delete had failed.
    let a2 = start(&b, "a").await;
    let stray = valka_wal::Lsn::new(1, 999);
    store
        .put(
            &valka_wal::lsn::keys::segment("a", stray),
            valka_wal::segment::encode(stray, "a", &[], true).unwrap(),
        )
        .await
        .unwrap();
    let report = a2.engine.gc_round().await.unwrap();
    assert_eq!(report.segments_deleted, 1);
    let per_shard = valka_wal::snapshot::list_all(&store).await.unwrap();
    assert!(per_shard.values().all(|v| v.len() <= 2));
}

#[tokio::test(start_paused = true)]
async fn a_takeover_does_not_wait_out_claim_backoff() {
    // Found by the failover benchmark: peers saw a dead node's lease expire by their own
    // clock and tried its shards before the guardian had split its log; each refusal
    // doubled a backoff (up to 30 s), so some shards were taken over 20 s after the split.
    let b = bucket();
    let a = start(&b, "a").await;
    let n2 = start(&b, "b").await;
    wait_until("converged", 20, || {
        a.engine.owned_shards().len() + n2.engine.owned_shards().len() == SHARDS as usize
            && !a.engine.owned_shards().is_empty()
    })
    .await;
    // One of a's shards, refused so often that its next attempt is a minute away.
    let shard = a.engine.owned_shards()[0];
    n2.engine.inner.membership.backoff.lock().insert(
        shard,
        (
            tokio::time::Instant::now() + Duration::from_secs(60),
            Duration::from_secs(30),
        ),
    );

    a.engine.halt();
    a.faults.set_puts_down(true);
    // lease_timeout is 3 s: the split and every claim follow within a few rounds.
    wait_until("b serves every shard", 8, || {
        n2.engine.owned_shards().len() == SHARDS as usize
    })
    .await;
    assert!(n2.engine.owns(shard));
}

#[tokio::test(start_paused = true)]
async fn a_claim_never_falls_back_past_an_unreadable_snapshot() {
    // Found by the code review: a newest snapshot that would not decode (truncated, or
    // written by a newer version) was skipped, and the claim went on from an older one or
    // from an empty shard, dropping records whose log may already be gone.
    let b = bucket();
    let a = start(&b, "a").await;
    a.engine.sync().await.unwrap();
    a.engine.halt();
    drop(a);
    let shard = ShardId(5);
    let generation = owners::get(&plain(&b), shard)
        .await
        .unwrap()
        .unwrap()
        .record
        .generation;
    let unreadable = valka_wal::snapshot::SnapshotKey {
        shard,
        generation,
        seq: 1_000,
        ptr: valka_wal::snapshot::LogPtr::Final,
    };
    plain(&b)
        .put(&unreadable.key(), bytes::Bytes::from_static(b"garbage"))
        .await
        .unwrap();

    let a2 = start(&b, "a").await;
    assert!(
        !a2.engine.owns(shard),
        "refused, not served from older state"
    );
    assert_eq!(a2.engine.owned_shards().len(), SHARDS as usize - 1);
}
