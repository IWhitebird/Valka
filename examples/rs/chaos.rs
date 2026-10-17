//! Chaos load generator and checker for a Valka cluster.
//!
//! Runs a worker on every node, creates tasks through every node (some with idempotency
//! keys), and records every create the cluster acknowledged. Meant to run while
//! `scripts/chaos.sh` kills, pauses and restarts nodes. When the load phase ends it waits
//! for every acknowledged task to finish and checks:
//!
//! - every acknowledged task exists (acked = durable),
//! - every acknowledged task reaches a terminal state (the cluster keeps serving),
//! - no idempotency key produced two tasks.
//!
//! It also reports how many tasks ran more than once (allowed, at-least-once; expected only
//! around failures). Exits non-zero on any violation.
//!
//! Environment:
//!   VALKA_CHAOS_GRPC     comma-separated gRPC addresses (default the compose cluster's)
//!   VALKA_CHAOS_SECONDS  load duration (default 120)
//!   VALKA_CHAOS_RATE     creates per second (default 20)
//!   VALKA_CHAOS_DRAIN    seconds to wait for completion after the load (default 180)
//!   VALKA_CHAOS_OUT      file to append acknowledged task ids to (optional)

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use valka_proto::CreateTaskRequest;
use valka_sdk::{ValkaClient, ValkaWorker};

const COMPLETED: i32 = 4;
const FAILED: i32 = 5;
const DEAD_LETTER: i32 = 7;
const CANCELLED: i32 = 8;

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A tiny xorshift RNG: no dependency, good enough to pick nodes and delays.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        Self(seed | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

async fn connect(addr: &str) -> Option<ValkaClient> {
    ValkaClient::connect(addr).await.ok()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let nodes: Vec<String> = std::env::var("VALKA_CHAOS_GRPC")
        .unwrap_or_else(|_| {
            "http://127.0.0.1:50061,http://127.0.0.1:50062,http://127.0.0.1:50063".into()
        })
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let seconds: u64 = env("VALKA_CHAOS_SECONDS", 120);
    let rate: u64 = env("VALKA_CHAOS_RATE", 20);
    let drain: u64 = env("VALKA_CHAOS_DRAIN", 180);
    let out = std::env::var("VALKA_CHAOS_OUT").ok();
    println!(
        "chaos: {} nodes, {seconds}s at {rate}/s, drain {drain}s",
        nodes.len()
    );

    // A worker on every node: tasks owned anywhere run anywhere.
    let executions: Arc<Mutex<HashMap<String, u32>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut stops = Vec::new();
    for (i, addr) in nodes.iter().enumerate() {
        let execs = executions.clone();
        let worker = ValkaWorker::builder()
            .name(&format!("chaos-worker-{i}"))
            .server_addr(addr)
            .queues(&["chaos"])
            .concurrency(8)
            .handler(move |ctx| {
                let execs = execs.clone();
                async move {
                    *execs.lock().await.entry(ctx.task_id.clone()).or_insert(0) += 1;
                    let input: serde_json::Value = ctx.input().unwrap_or_default();
                    let ms = input["ms"].as_u64().unwrap_or(100);
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    if input["fail"] == true && ctx.attempt_number == 1 {
                        return Err("first attempt fails on purpose".to_string());
                    }
                    Ok(serde_json::json!({ "slept_ms": ms }))
                }
            })
            .build()
            .await
            .expect("worker");
        stops.push(worker.shutdown_handle());
        tokio::spawn(worker.run());
    }

    // Load: create through random nodes, remember what was acknowledged. Creates run
    // concurrently (bounded), so a frozen node slows only the creates sent to it.
    let acked: Arc<Mutex<HashMap<String, Option<String>>>> = Arc::new(Mutex::new(HashMap::new()));
    let errors = Arc::new(AtomicU64::new(0));
    let file = Arc::new(std::sync::Mutex::new(out.map(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .expect("output file")
    })));
    let mut initial = Vec::new();
    for addr in &nodes {
        initial.push(connect(addr).await);
    }
    let clients: Arc<std::sync::Mutex<Vec<Option<ValkaClient>>>> =
        Arc::new(std::sync::Mutex::new(initial));
    let reconnecting: Arc<std::sync::Mutex<HashSet<usize>>> = Default::default();
    const IN_FLIGHT: usize = 128;
    let slots = Arc::new(tokio::sync::Semaphore::new(IN_FLIGHT));
    let mut rng = Rng::new();
    let start = Instant::now();
    let mut n = 0u64;
    let period = Duration::from_micros(1_000_000 / rate.max(1));
    let mut tick = tokio::time::interval(period);
    while start.elapsed() < Duration::from_secs(seconds) {
        tick.tick().await;
        n += 1;
        if n.is_multiple_of((rate * 10).max(1)) {
            println!(
                "  t={:>4}s created {} (errors {}, in flight {})",
                start.elapsed().as_secs(),
                acked.lock().await.len(),
                errors.load(Ordering::Relaxed),
                IN_FLIGHT - slots.available_permits()
            );
        }
        let i = rng.below(nodes.len() as u64) as usize;
        let client = clients.lock().unwrap()[i].clone();
        let Some(mut client) = client else {
            // Not connected: count it as a failed create and reconnect in the background.
            errors.fetch_add(1, Ordering::Relaxed);
            if reconnecting.lock().unwrap().insert(i) {
                let (addr, clients, reconnecting) =
                    (nodes[i].clone(), clients.clone(), reconnecting.clone());
                tokio::spawn(async move {
                    if let Ok(Some(c)) =
                        tokio::time::timeout(Duration::from_secs(5), connect(&addr)).await
                    {
                        clients.lock().unwrap()[i] = Some(c);
                    }
                    reconnecting.lock().unwrap().remove(&i);
                });
            }
            continue;
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            errors.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let key = (rng.below(10) == 0).then(|| format!("chaos-key-{}", n / 3));
        let input = serde_json::json!({
            "ms": 20 + rng.below(400),
            "fail": rng.below(20) == 0,
        });
        let req = CreateTaskRequest {
            queue_name: "chaos".into(),
            task_name: "work".into(),
            input: input.to_string(),
            priority: 0,
            max_retries: 5,
            timeout_seconds: 30,
            idempotency_key: key.clone().unwrap_or_default(),
            metadata: String::new(),
            scheduled_at: String::new(),
        };
        let (acked, errors, file, clients) =
            (acked.clone(), errors.clone(), file.clone(), clients.clone());
        tokio::spawn(async move {
            let _permit = permit;
            match tokio::time::timeout(Duration::from_secs(10), client.create_task_request(req))
                .await
            {
                Ok(Ok(t)) => {
                    if let Some(f) = file.lock().unwrap().as_mut() {
                        let _ = writeln!(f, "{}", t.id);
                    }
                    acked.lock().await.insert(t.id, key);
                }
                // A duplicate key is a correct refusal, not an error.
                Ok(Err(e)) if e.to_string().contains("dempotency") => {}
                Ok(Err(_)) | Err(_) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    clients.lock().unwrap()[i] = None;
                }
            }
        });
    }
    // Let the creates still in flight finish (each gives up after 10 s).
    let _all = slots.acquire_many(IN_FLIGHT as u32).await;
    let acked = acked.lock().await.clone();
    println!(
        "load done: {} acknowledged creates, {} failed creates (not acknowledged)",
        acked.len(),
        errors.load(Ordering::Relaxed)
    );
    let mut clients: Vec<Option<ValkaClient>> = clients.lock().unwrap().clone();

    // Check: every acknowledged task exists and finishes.
    let deadline = Instant::now() + Duration::from_secs(drain);
    let mut pending: HashSet<String> = acked.keys().cloned().collect();
    let mut missing: HashSet<String> = HashSet::new();
    let mut statuses: HashMap<i32, usize> = HashMap::new();
    let mut by_key: HashMap<String, HashSet<String>> = HashMap::new();
    while !pending.is_empty() && Instant::now() < deadline {
        let batch: Vec<String> = pending.iter().take(200).cloned().collect();
        for id in batch {
            let i = rng.below(nodes.len() as u64) as usize;
            if clients[i].is_none() {
                clients[i] = connect(&nodes[i]).await;
            }
            let Some(client) = clients[i].as_mut() else {
                continue;
            };
            match tokio::time::timeout(Duration::from_secs(10), client.get_task(&id)).await {
                Err(_) => clients[i] = None,
                Ok(Ok(t)) => {
                    missing.remove(&id);
                    if !t.idempotency_key.is_empty() {
                        by_key
                            .entry(t.idempotency_key.clone())
                            .or_default()
                            .insert(t.id.clone());
                    }
                    if matches!(t.status, COMPLETED | FAILED | DEAD_LETTER | CANCELLED) {
                        *statuses.entry(t.status).or_default() += 1;
                        pending.remove(&id);
                    }
                }
                Ok(Err(e)) if e.to_string().to_lowercase().contains("not found") => {
                    missing.insert(id);
                }
                Ok(Err(_)) => clients[i] = None,
            }
        }
        if !pending.is_empty() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    for s in &stops {
        s.shutdown();
    }

    let execs = executions.lock().await;
    let reran = acked
        .keys()
        .filter(|id| execs.get(*id).copied().unwrap_or(0) > 1)
        .count();
    let dup_keys: Vec<&String> = by_key
        .iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(k, _)| k)
        .collect();
    println!("result:");
    println!("  acknowledged       {}", acked.len());
    println!(
        "  completed          {}",
        statuses.get(&COMPLETED).unwrap_or(&0)
    );
    println!(
        "  dead-lettered      {}",
        statuses.get(&DEAD_LETTER).unwrap_or(&0)
    );
    println!(
        "  failed / cancelled {}",
        statuses.get(&FAILED).unwrap_or(&0) + statuses.get(&CANCELLED).unwrap_or(&0)
    );
    println!("  not finished       {}", pending.len());
    println!("  missing (LOST)     {}", missing.len());
    println!("  duplicate keys     {}", dup_keys.len());
    println!("  ran more than once {reran} (at-least-once; includes deliberate retries)");
    let ok = missing.is_empty() && pending.is_empty() && dup_keys.is_empty();
    if !ok {
        for id in missing.iter().take(10) {
            println!("  lost: {id}");
        }
        for id in pending.iter().take(10) {
            println!("  unfinished: {id}");
        }
        println!("FAIL");
        std::process::exit(1);
    }
    println!("PASS");
}
