//! Load generator that measures a Valka node or cluster.
//!
//! Producers keep a fixed number of creates in flight for a while; a worker on every node
//! runs each task at once. It reports:
//!
//! - creates per second, and the create latency (request sent → task durable and
//!   acknowledged) at p50/p95/p99/max;
//! - the pickup latency (create sent → a worker's handler starts) at the same percentiles;
//! - errors, and whether every acknowledged task ran.
//!
//! Environment:
//!   VALKA_BENCH_GRPC       comma-separated gRPC addresses (default http://127.0.0.1:50051)
//!   VALKA_BENCH_SECONDS    how long to create (default 30)
//!   VALKA_BENCH_INFLIGHT   creates in flight at once (default 64)
//!   VALKA_BENCH_WORKER_CONCURRENCY   tasks each node's worker runs at once (default 64)
//!   VALKA_BENCH_PAYLOAD    bytes of input per task (default 100)
//!   VALKA_API_KEY          when the server requires one
//!
//! Build it in release mode: `cargo run --release -p valka-examples --example bench`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use valka_proto::CreateTaskRequest;
use valka_sdk::{ValkaClient, ValkaWorker};

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64
}

/// p50, p95, p99 and max of `v` (microseconds), in milliseconds.
fn percentiles(v: &mut [u64]) -> [f64; 4] {
    if v.is_empty() {
        return [0.0; 4];
    }
    v.sort_unstable();
    let at = |q: f64| v[((v.len() as f64 * q) as usize).min(v.len() - 1)] as f64 / 1000.0;
    [
        at(0.50),
        at(0.95),
        at(0.99),
        *v.last().unwrap() as f64 / 1000.0,
    ]
}

#[tokio::main]
async fn main() {
    let nodes: Vec<String> = std::env::var("VALKA_BENCH_GRPC")
        .unwrap_or_else(|_| "http://127.0.0.1:50051".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let seconds: u64 = env("VALKA_BENCH_SECONDS", 30);
    let inflight: usize = env("VALKA_BENCH_INFLIGHT", 64);
    let worker_concurrency: i32 = env("VALKA_BENCH_WORKER_CONCURRENCY", 64);
    let payload: usize = env("VALKA_BENCH_PAYLOAD", 100);
    let queue = format!("bench-{}", now_micros() % 1_000_000);
    println!(
        "bench: {} node(s), {seconds} s, {inflight} creates in flight, workers x{worker_concurrency}, {payload}-byte inputs, queue {queue}",
        nodes.len()
    );

    // One worker per node: tasks of any node run anywhere.
    let pickups: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let ran = Arc::new(AtomicU64::new(0));
    let mut stops = Vec::new();
    for (i, addr) in nodes.iter().enumerate() {
        let (pickups, ran) = (pickups.clone(), ran.clone());
        let worker = ValkaWorker::builder()
            .name(&format!("bench-worker-{i}"))
            .server_addr(addr)
            .queues(&[&queue])
            .concurrency(worker_concurrency)
            .handler(move |ctx| {
                let (pickups, ran) = (pickups.clone(), ran.clone());
                async move {
                    let input: serde_json::Value = ctx.input().unwrap_or_default();
                    if let Some(sent) = input["sent_us"].as_u64() {
                        pickups.lock().await.push(now_micros().saturating_sub(sent));
                    }
                    ran.fetch_add(1, Ordering::Relaxed);
                    Ok(serde_json::json!({}))
                }
            })
            .build()
            .await
            .expect("worker");
        stops.push(worker.shutdown_handle());
        tokio::spawn(worker.run());
    }
    // Let the workers attach before the clock starts.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let latencies: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let errors = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let filler = "x".repeat(payload);
    let mut producers = Vec::new();
    for p in 0..inflight {
        // Spread producers over the nodes, like clients behind a load balancer.
        let mut order = nodes.clone();
        order.rotate_left(p % nodes.len());
        let mut client = ValkaClient::builder()
            .addrs(&order)
            .connect()
            .await
            .expect("client");
        let (latencies, errors, stop, queue, filler) = (
            latencies.clone(),
            errors.clone(),
            stop.clone(),
            queue.clone(),
            filler.clone(),
        );
        producers.push(tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let sent = now_micros();
                let t0 = Instant::now();
                let req = CreateTaskRequest {
                    queue_name: queue.clone(),
                    task_name: "bench".into(),
                    input: serde_json::json!({ "sent_us": sent, "pad": filler }).to_string(),
                    max_retries: 3,
                    timeout_seconds: 60,
                    ..Default::default()
                };
                match client.create_task_request(req).await {
                    Ok(_) => latencies.lock().await.push(t0.elapsed().as_micros() as u64),
                    Err(_) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    let started = Instant::now();
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    stop.store(true, Ordering::Relaxed);
    for p in producers {
        let _ = p.await;
    }
    let elapsed = started.elapsed().as_secs_f64();
    let mut lat = latencies.lock().await.clone();
    let created = lat.len() as u64;

    // Wait for the workers to finish what was created.
    let deadline = Instant::now() + Duration::from_secs(60);
    while ran.load(Ordering::Relaxed) < created && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for s in &stops {
        s.shutdown();
    }
    let mut pick = pickups.lock().await.clone();
    let c = percentiles(&mut lat);
    let k = percentiles(&mut pick);
    let rate = created as f64 / elapsed;
    println!(
        "creates      {created} in {elapsed:.1} s = {rate:.0}/s, {} errors",
        errors.load(Ordering::Relaxed)
    );
    println!(
        "create ack   p50 {:.1} ms  p95 {:.1} ms  p99 {:.1} ms  max {:.1} ms",
        c[0], c[1], c[2], c[3]
    );
    println!(
        "pickup       p50 {:.1} ms  p95 {:.1} ms  p99 {:.1} ms  max {:.1} ms",
        k[0], k[1], k[2], k[3]
    );
    println!(
        "ran          {} of {created}{}",
        ran.load(Ordering::Relaxed),
        if ran.load(Ordering::Relaxed) >= created {
            ""
        } else {
            " (some still pending)"
        }
    );
    println!(
        "{}",
        serde_json::json!({
            "nodes": nodes.len(), "seconds": elapsed, "inflight": inflight,
            "created": created, "rate_per_s": rate, "errors": errors.load(Ordering::Relaxed),
            "create_ms": {"p50": c[0], "p95": c[1], "p99": c[2], "max": c[3]},
            "pickup_ms": {"p50": k[0], "p95": k[1], "p99": k[2], "max": k[3]},
            "ran": ran.load(Ordering::Relaxed),
        })
    );
}
