//! Randomized multi-node simulation with a history checker (PHASE2 §15.2–15.3, in-process).
//!
//! Several engines share one in-memory bucket, each through its own fault injector.
//! A seeded random schedule creates tasks (keyed and not), completes and fails runs, kills,
//! restarts, freezes and drains nodes, and turns on bucket faults, while tokio time is
//! paused so leases, takeovers and timers run deterministically and fast. Every
//! acknowledged operation is recorded. At the end the faults are healed, the cluster
//! converges, and the history is checked:
//!
//! - **G1 acked = durable:** every acknowledged create exists, every acknowledged
//!   completion is still COMPLETED, every acknowledged non-retryable failure is FAILED;
//! - **G2 one history per shard:** every shard is served by exactly one live node;
//! - **idempotency:** at most one task per idempotency key, and an acknowledged keyed
//!   create is the one that exists.
//!
//! `VALKA_SIM_SEEDS=N` runs more seeds (default 8), from `VALKA_SIM_SEED` (default 0).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use rand::{RngExt, SeedableRng, rngs::StdRng};
use valka_core::{ShardId, TaskStatus};
use valka_wal::fault::faulty_over;
use valka_wal::{FaultConfig, Store};

use crate::clock::TokioClock;
use crate::engine::{CreateTask, Engine, EngineConfig};

const SHARDS: u32 = 32;
const NODES: usize = 3;

fn cfg(node: &str) -> EngineConfig {
    let mut c = EngineConfig::for_tests(node);
    c.cluster.num_shards = SHARDS;
    c.cluster.lease_interval_ms = 500;
    c.cluster.lease_timeout_ms = 3_000;
    c.cluster.lease_safety_margin_ms = 1_000;
    c.cluster.membership_interval_ms = 300;
    c.cluster.placement_interval_ms = 200;
    c.cluster.split_timeout_ms = 4_000;
    c.wal.put_retries = 3;
    c.wal.snapshot_interval_secs = 2;
    c
}

struct SimNode {
    name: String,
    engine: Option<Engine>,
    faults: Arc<FaultConfig>,
    frozen: bool,
}

#[derive(Default)]
struct History {
    /// Acknowledged creates: task id -> idempotency key.
    created: HashMap<String, Option<String>>,
    completed: HashSet<String>,
    failed: HashSet<String>,
    acked_keys: HashMap<String, String>,
}

fn req(queue: &str, key: Option<String>) -> CreateTask {
    CreateTask {
        queue_name: queue.into(),
        task_name: "t".into(),
        input: None,
        priority: 0,
        max_retries: 2,
        timeout_seconds: 60,
        idempotency_key: key,
        metadata: serde_json::json!({}),
        scheduled_at: None,
    }
}

async fn start(backing: &Arc<dyn object_store::ObjectStore>, name: &str, seed: u64) -> SimNode {
    let (store, faults) = faulty_over(backing.clone(), seed);
    let engine = open_retrying(store, name).await;
    SimNode {
        name: name.to_string(),
        engine: Some(engine),
        faults,
        frozen: false,
    }
}

async fn open_retrying(store: Store, name: &str) -> Engine {
    for _ in 0..20 {
        match Engine::open_with(
            store.clone(),
            cfg(name),
            TokioClock::new(),
            Arc::new(crate::sink::NoopSink),
        )
        .await
        {
            Ok(e) => return e,
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    panic!("{name} could not start");
}

fn live(nodes: &[SimNode]) -> Vec<&Engine> {
    nodes
        .iter()
        .filter(|n| !n.frozen)
        .filter_map(|n| n.engine.as_ref())
        .filter(|e| e.poisoned().is_none() && !e.is_drained())
        .collect()
}

fn owner<'a>(nodes: &'a [SimNode], id: &str) -> Option<&'a Engine> {
    live(nodes)
        .into_iter()
        .find(|e| e.shard_of(id).is_some_and(|s| e.owns(s)))
}

#[allow(clippy::needless_range_loop)]
async fn run_seed(seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let backing: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let mut nodes = Vec::new();
    for i in 0..NODES {
        nodes.push(start(&backing, &format!("n{i}"), seed * 10 + i as u64).await);
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut h = History::default();
    let mut running: Vec<(String, String)> = Vec::new();
    let mut key_seq = 0u32;

    for step in 0..120 {
        match rng.random_range(0..100) {
            // Create, through whichever node owns the chosen shard.
            0..=44 => {
                let key = rng.random_bool(0.3).then(|| {
                    key_seq += 1;
                    format!("k{}", key_seq % 20)
                });
                let candidates = live(&nodes);
                if candidates.is_empty() {
                    continue;
                }
                let e = candidates[rng.random_range(0..candidates.len())];
                // An error is not an acknowledgement: that task may or may not exist.
                if let Ok(t) = e.create_task(req("q", key.clone())).await {
                    if let Some(k) = &key {
                        h.acked_keys.insert(k.clone(), t.id.clone());
                    }
                    h.created.insert(t.id, key);
                }
            }
            // Dispatch a pending task on its owner.
            45..=59 => {
                let ids: Vec<String> = h.created.keys().cloned().collect();
                if ids.is_empty() {
                    continue;
                }
                let id = &ids[rng.random_range(0..ids.len())];
                if let Some(e) = owner(&nodes, id)
                    && let Ok(d) = e.dispatch(id, "w")
                {
                    running.push((id.clone(), d.run_id));
                }
            }
            // Finish a run: complete, or fail for good.
            60..=74 => {
                if running.is_empty() {
                    continue;
                }
                let (id, run) = running.swap_remove(rng.random_range(0..running.len()));
                let Some(e) = owner(&nodes, &id) else {
                    continue;
                };
                if rng.random_bool(0.8) {
                    if e.complete_run(&id, &run, None).await.is_ok() {
                        h.completed.insert(id);
                    }
                } else if e.fail_run(&id, &run, "boom", false).await.is_ok() {
                    h.failed.insert(id);
                }
            }
            // Kill a node (kill -9), or restart a dead one with a new incarnation.
            75..=81 => {
                let i = rng.random_range(0..NODES);
                if nodes[i].engine.is_some() && !nodes[i].frozen && live(&nodes).len() > 1 {
                    let e = nodes[i].engine.take().unwrap();
                    e.halt();
                    nodes[i].faults.set_puts_down(true);
                } else if nodes[i].engine.is_none() {
                    let name = nodes[i].name.clone();
                    nodes[i] = start(&backing, &name, seed * 100 + step).await;
                }
            }
            // Freeze a node (GC pause, SIGSTOP), or wake it up.
            82..=86 => {
                let i = rng.random_range(0..NODES);
                if nodes[i].engine.is_some() {
                    if nodes[i].frozen {
                        nodes[i].faults.freeze(false);
                        nodes[i].frozen = false;
                    } else if live(&nodes).len() > 1 {
                        nodes[i].faults.freeze(true);
                        nodes[i].frozen = true;
                    }
                }
            }
            // Drain a node (rolling restart), then start it again.
            87..=89 => {
                let i = rng.random_range(0..NODES);
                if let Some(e) = nodes[i].engine.clone()
                    && !nodes[i].frozen
                    && live(&nodes).len() > 1
                {
                    let _ = e.drain().await;
                    e.halt();
                    let name = nodes[i].name.clone();
                    nodes[i] = start(&backing, &name, seed * 1000 + step).await;
                }
            }
            // Bucket trouble on one node for a while.
            90..=94 => {
                let i = rng.random_range(0..NODES);
                nodes[i].faults.set_fail_rate(rng.random_range(50..300));
                nodes[i]
                    .faults
                    .set_lost_put_ack_rate(rng.random_range(0..200));
            }
            _ => {
                for n in &nodes {
                    n.faults.set_fail_rate(0);
                    n.faults.set_lost_put_ack_rate(0);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(rng.random_range(10..400))).await;
        // A node whose writer gave up exits; its supervisor restarts it.
        for i in 0..NODES {
            let poisoned = nodes[i]
                .engine
                .as_ref()
                .is_some_and(|e| e.poisoned().is_some());
            if poisoned && !nodes[i].frozen {
                let e = nodes[i].engine.take().unwrap();
                e.halt();
                nodes[i].faults.set_puts_down(true);
            }
        }
    }

    // Heal: wake everyone, clear faults, restart every dead node, converge.
    for i in 0..NODES {
        nodes[i].faults.freeze(false);
        nodes[i].frozen = false;
        nodes[i].faults.set_fail_rate(0);
        nodes[i].faults.set_lost_put_ack_rate(0);
        let dead = nodes[i]
            .engine
            .as_ref()
            .is_none_or(|e| e.poisoned().is_some() || e.is_drained());
        if dead {
            if let Some(e) = nodes[i].engine.take() {
                e.halt();
            }
            let name = nodes[i].name.clone();
            nodes[i] = start(&backing, &name, seed * 7 + 99).await;
        }
    }
    let mut converged = false;
    for round in 0..300 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        // The supervisor restarts a node that exits (fenced after waking up, poisoned).
        for i in 0..NODES {
            let exited = nodes[i]
                .engine
                .as_ref()
                .is_none_or(|e| e.poisoned().is_some());
            if exited {
                if let Some(e) = nodes[i].engine.take() {
                    e.halt();
                }
                let name = nodes[i].name.clone();
                nodes[i] = start(&backing, &name, seed * 13 + round).await;
            }
        }
        let served: usize = live(&nodes).iter().map(|e| e.owned_shards().len()).sum();
        if served == SHARDS as usize && live(&nodes).len() == NODES {
            converged = true;
            break;
        }
    }
    if !converged {
        dump(&backing, &nodes).await;
    }
    assert!(
        converged,
        "seed {seed}: the cluster did not converge after healing"
    );

    // G2: one live owner per shard.
    let mut owners: BTreeMap<ShardId, String> = BTreeMap::new();
    for e in live(&nodes) {
        for s in e.owned_shards() {
            if let Some(other) = owners.insert(s, e.node_id().to_string()) {
                panic!(
                    "seed {seed}: shard {s} served by {other} and {}",
                    e.node_id()
                );
            }
        }
    }
    // G1: every acknowledged create exists; acknowledged ends hold.
    for id in h.created.keys() {
        let e = owner(&nodes, id).unwrap_or_else(|| panic!("seed {seed}: no owner for {id}"));
        let t = e
            .get_task(id)
            .unwrap_or_else(|| panic!("seed {seed}: acknowledged task {id} lost"));
        if h.completed.contains(id) {
            assert_eq!(t.status, TaskStatus::Completed, "seed {seed}: task {id}");
        }
        if h.failed.contains(id) {
            assert_eq!(t.status, TaskStatus::Failed, "seed {seed}: task {id}");
        }
    }
    // Idempotency: one task per key, and the acknowledged one.
    let mut by_key: HashMap<String, Vec<String>> = HashMap::new();
    for e in live(&nodes) {
        for t in e.list_tasks(Some("q"), None, 10_000, 0) {
            if let Some(k) = t.idempotency_key {
                by_key.entry(k).or_default().push(t.id);
            }
        }
    }
    for (k, ids) in &by_key {
        assert_eq!(ids.len(), 1, "seed {seed}: key {k} created twice: {ids:?}");
        if let Some(acked) = h.acked_keys.get(k) {
            assert_eq!(&ids[0], acked, "seed {seed}: key {k}");
        }
    }
    for k in h.acked_keys.keys() {
        assert!(
            by_key.contains_key(k),
            "seed {seed}: acknowledged key {k} lost"
        );
    }
    for n in &nodes {
        if let Some(e) = &n.engine {
            e.halt();
        }
    }
}

/// What the bucket and the nodes believe, when a run does not converge.
async fn dump(backing: &Arc<dyn object_store::ObjectStore>, nodes: &[SimNode]) {
    let store = Store::wrap(backing.clone(), "", true, "memory");
    for n in nodes {
        match &n.engine {
            Some(e) => eprintln!(
                "node {} inc {} serving={} poisoned={:?} drained={} owns={} eligible={:?} members={:?}",
                n.name,
                e.incarnation(),
                e.is_serving(),
                e.poisoned(),
                e.is_drained(),
                e.owned_shards().len(),
                e.eligible_nodes(),
                e.members()
                    .iter()
                    .map(|m| (m.node_id.clone(), m.inc, m.state.clone(), m.live, m.suspect))
                    .collect::<Vec<_>>()
            ),
            None => eprintln!("node {} down", n.name),
        }
    }
    for (node, _) in valka_wal::lease::list(&store).await.unwrap() {
        let l = valka_wal::lease::get(&store, &node)
            .await
            .unwrap()
            .unwrap()
            .record;
        eprintln!(
            "lease {node}: inc {} {:?} trunc {} unsplit {:?} dead_by {:?}",
            l.inc, l.state, l.trunc, l.unsplit, l.dead_by
        );
    }
    let markers: Vec<String> = store
        .list("splits/")
        .await
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    eprintln!("markers {markers:?}");
    for s in ShardId::all(SHARDS) {
        if nodes
            .iter()
            .filter_map(|n| n.engine.as_ref())
            .any(|e| e.owns(s))
        {
            continue;
        }
        let o = valka_wal::owners::get(&store, s)
            .await
            .unwrap()
            .map(|v| v.record);
        let snaps = valka_wal::snapshot::list(&store, s).await.unwrap();
        eprintln!(
            "unowned shard {s}: owner {o:?} newest snapshot {:?}",
            snaps.last().map(|k| k.key())
        );
    }
}

#[tokio::test(start_paused = true)]
#[allow(clippy::needless_range_loop)]
async fn randomized_cluster_history_keeps_every_acknowledgement() {
    let seeds: u64 = std::env::var("VALKA_SIM_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let first: u64 = std::env::var("VALKA_SIM_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    for seed in first..first + seeds {
        run_seed(seed).await;
    }
}
