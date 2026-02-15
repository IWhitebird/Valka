<p align="center">
  <img src="assets/banner.svg" alt="Valka — Distributed Task Queue" width="700"/>
</p>

<p align="center">
  <strong>A Rust-native distributed task queue powered by PostgreSQL.</strong><br/>
  One dependency. Zero brokers. Built for simplicity.
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#sdks">SDKs</a> &bull;
  <a href="#architecture">Architecture</a> &bull;
  <a href="#web-dashboard">Dashboard</a> &bull;
  <a href="#deployment">Deployment</a>
</p>

<p align="center">
  <img alt="License" src="https://img.shields.io/badge/license-Apache%202.0-blue.svg"/>
</p>

---

## Why Valka?

Most task queues bolt together a message broker, a database, and a cache. Every moving part is another thing to deploy, monitor, and debug at 3 AM.

**Valka takes a different approach.** PostgreSQL is the single source of truth. An in-memory matching engine and gRPC bidirectional streaming replace the message broker entirely. The result: simple to operate, easy to reason about, and fast where it matters.

- **One dependency.** If you have Postgres, you can run Valka.
- **Zero-latency hot path.** Tasks are matched to waiting workers in-memory — no polling.
- **Polyglot.** Rust, TypeScript, Go, and Python SDKs. Or just use the REST API.
- **Observable.** Real-time log streaming, event feeds, Prometheus metrics, and a web dashboard out of the box.

## Features

- In-memory task matching with PG `SKIP LOCKED` fallback
- gRPC bidirectional streaming (single connection per worker, no polling)
- Multi-node clustering with chitchat gossip + consistent hash ring
- Task signals — send real-time signals to running workers
- Automatic retries with exponential backoff + dead letter queue
- Structured log streaming per task run
- Event broadcasting via gRPC streams and SSE
- Idempotency keys, task cancellation, scheduled/delayed tasks
- Web dashboard, CLI, and Prometheus metrics
- Graceful shutdown for both server and workers

## Quick Start

```bash
# 1. Start PostgreSQL
docker compose up -d postgres

# 2. Start the server (runs migrations automatically)
cargo run -p valka-server

# 3. Run a worker
cargo run -p valka-examples --example worker

# 4. Create tasks
cargo run -p valka-examples --example producer
```

The server starts on **gRPC `:50051`** and **REST `:8989`**.

## SDKs

| Language | Package | Install |
|----------|---------|---------|
| **Rust** | `valka-sdk` | `cargo add valka-sdk` |
| **TypeScript** | `@valka/sdk` | `npm install @valka/sdk` |
| **Go** | `github.com/valka-queue/valka/sdks/go` | `go get github.com/valka-queue/valka/sdks/go` |
| **Python** | `valka` | `pip install valka` |

Any language can also interact via the REST API.

### Rust

```rust
let worker = ValkaWorker::builder()
    .name("email-worker")
    .server_addr("http://localhost:50051")
    .queues(&["emails"])
    .concurrency(8)
    .handler(|ctx: TaskContext| async move {
        let input: serde_json::Value = ctx.input()?;
        // ... do work ...
        Ok(serde_json::json!({"status": "delivered"}))
    })
    .build()
    .await?;

worker.run().await
```

### TypeScript

```typescript
const worker = new ValkaWorker({
  name: "email-worker",
  serverAddr: "localhost:50051",
  queues: ["emails"],
  concurrency: 8,
  handler: async (ctx) => {
    console.log(`Processing: ${ctx.taskName}`);
    return { status: "delivered" };
  },
});
await worker.run();
```

## Architecture

```
                    ┌─────────────────────────────────────────────┐
                    │                VALKA SERVER                  │
                    │                                             │
  REST clients ───► │  REST API ──┐                               │
                    │             ├──► MatchingService             │
  gRPC clients ───► │  gRPC API ──┘     (partition tree)          │
                    │                       │                     │
                    │               ┌───────┴───────┐             │
                    │               ▼               ▼             │
                    │          Hot Path         Cold Path          │
                    │       (in-memory         (PG SKIP           │
                    │        oneshot)           LOCKED)            │
                    │               │               │             │
                    │               └───────┬───────┘             │
                    │                       ▼                     │
                    │                  Dispatcher                  │
                    │              (gRPC bidi stream)              │
                    │                       │                     │
                    │   ┌───────────────────┼───────────────────┐ │
                    │   │  Scheduler        │                   │ │
                    │   │  ├─ Lease Reaper  │  Event Broadcast  │ │
                    │   │  ├─ Retry Engine  │  (tokio broadcast)│ │
                    │   │  ├─ DLQ Mover     │                   │ │
                    │   │  └─ Delayed Promo │                   │ │
                    │   └───────────────────┼───────────────────┘ │
                    └───────────────────────┼─────────────────────┘
                                            │
                              ┌─────────────┼─────────────┐
                              ▼             ▼             ▼
                          Worker A      Worker B      Worker C
                         (Rust SDK)    (Go SDK)    (Python SDK)
```

### Task Lifecycle

```
PENDING ──► DISPATCHING ──► RUNNING ──┬──► COMPLETED
                                      ├──► FAILED
                                      └──► RETRY ──┬──► PENDING (rescheduled)
                                                    └──► DEAD_LETTER (exhausted)
CANCELLED (via API at any time)
```

## Web Dashboard

Valka ships with a built-in React dashboard at the root path.

```bash
cd web && npm install && npm run dev  # Dev server on :5173
```

Pages: Dashboard, Tasks, Task Detail (runs, logs, signals), Workers, Events, Dead Letters.

## Deployment

### Docker Compose (recommended)

```bash
docker compose up
```

Starts PostgreSQL 17 + Valka server. REST + Dashboard on `:8989`, gRPC on `:50051`.

### From Source

```bash
cargo build --release --workspace
./target/release/valka-server
```

### Configuration

Layered via [figment](https://github.com/SergioBenitez/Figment): defaults → `valka.toml` → env vars.

| Variable | Default | Description |
|----------|---------|-------------|
| `VALKA_DATABASE_URL` | — | PostgreSQL connection string |
| `VALKA_GRPC_ADDR` | `0.0.0.0:50051` | gRPC listen address |
| `VALKA_HTTP_ADDR` | `0.0.0.0:8989` | REST/HTTP listen address |
| `RUST_LOG` | `valka=info,tower_http=info` | Log level filter |

## Roadmap

### Done

- [x] Core task queue with retry and dead letter queue
- [x] gRPC bidirectional streaming workers
- [x] REST API + CLI + Web dashboard
- [x] Real-time event and log streaming
- [x] Task signals (send signals to running workers)
- [x] Multi-node clustering (chitchat gossip + consistent hash ring)
- [x] Polyglot SDKs — Rust, TypeScript, Go, Python

### Up Next

- [ ] Task priorities and weighted fair queuing
- [ ] Cron / recurring task scheduling
- [ ] Rate limiting per queue
- [ ] Task batching

## License

Apache License 2.0 — see [LICENSE](LICENSE) for details.
