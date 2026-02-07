<p align="center">
  <img src="assets/banner.svg" alt="Valka — Distributed Task Queue" width="700"/>
</p>

<p align="center">
  <strong>A Rust-native distributed task queue powered by PostgreSQL.</strong><br/>
  One dependency. Zero brokers. Built for simplicity.
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#features">Features</a> &bull;
  <a href="#architecture">Architecture</a> &bull;
  <a href="#sdks">SDKs</a> &bull;
  <a href="#web-dashboard">Dashboard</a> &bull;
  <a href="#deployment">Deployment</a>
</p>

<p align="center">
  <img alt="License" src="https://img.shields.io/badge/license-Apache%202.0-blue.svg"/>
</p>

---

## Why Valka?

Most task queues bolt together multiple infrastructure services — a message broker for dispatch, a database for persistence, a cache for state. Every additional moving part is another thing to deploy, monitor, and debug at 3 AM.

**Valka takes a different approach.** It uses PostgreSQL as the single source of truth and replaces the message broker entirely with gRPC bidirectional streaming and an in-memory matching engine. The result is a task queue that is simple to operate, easy to reason about, and fast where it matters.

- **One dependency.** If you have Postgres, you can run Valka.
- **Zero-latency hot path.** Tasks are matched to waiting workers in-memory before they even hit the database poll loop.
- **Polyglot by default.** Rust and TypeScript SDKs included. Any language that speaks REST or gRPC can participate.
- **Built for observability.** Real-time log streaming, SSE event feeds, Prometheus metrics, and a web dashboard — out of the box.

## Features

| | |
|---|---|
| **In-Memory Task Matching** | Partition tree routes tasks to waiting workers instantly via oneshot channels. Cold-path fallback uses PG `SKIP LOCKED` for crash-safe dequeuing. |
| **gRPC Bidirectional Streaming** | Single persistent connection per worker. No polling. Tasks are pushed the moment they're ready. |
| **Automatic Retries** | Exponential backoff with jitter, configurable max retries and delay caps. Tasks that exhaust retries land in the dead letter queue. |
| **Real-Time Log Streaming** | Workers emit structured logs that are queryable per task run. Tail logs via the CLI, REST SSE, or the web dashboard. |
| **Event Broadcasting** | Every task state transition emits an event. Subscribe via gRPC stream or Server-Sent Events. |
| **Idempotency** | Optional idempotency keys ensure exactly-once task creation. |
| **Task Cancellation** | Cancel pending or running tasks. Running workers receive a cancellation signal over the stream. |
| **Dead Letter Queue** | Failed tasks are preserved with full context for inspection and replay. |
| **Web Dashboard** | React-based UI for monitoring queues, inspecting tasks, viewing workers, and tailing logs. |
| **Prometheus Metrics** | `/metrics` endpoint for scraping queue depths, latencies, and worker counts. |
| **Graceful Shutdown** | Workers drain in-flight tasks on SIGINT. The server notifies all connected workers before shutting down. |
| **Scheduled Tasks** | Set `scheduled_at` to defer task execution to a future time. |

## Quick Start

### Prerequisites

- [Rust 1.85+](https://rustup.rs/)
- [Docker](https://docs.docker.com/get-docker/) (for PostgreSQL)

### 1. Start PostgreSQL

```bash
docker compose up -d postgres
```

### 2. Run the Server

```bash
cargo run -p valka-server
```

The server starts on **gRPC `:50051`** and **REST `:8080`**, and runs migrations automatically.

### 3. Run a Worker

```bash
cargo run -p valka-examples --example worker
```

### 4. Create Tasks

```bash
cargo run -p valka-examples --example producer
```

Or use the CLI:

```bash
cargo run -p valka-cli -- task create \
  --queue emails \
  --name send-welcome \
  --input '{"to": "user@example.com", "template": "welcome"}'
```

Or use curl:

```bash
curl -X POST http://localhost:8080/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "queue_name": "emails",
    "task_name": "send-welcome",
    "input": {"to": "user@example.com", "template": "welcome"}
  }'
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
                         (Rust SDK)   (TS SDK)     (any gRPC)
```

### Task Lifecycle

```
PENDING ──► DISPATCHING ──► RUNNING ──┬──► COMPLETED
                                      ├──► FAILED
                                      └──► RETRY ──┬──► PENDING (rescheduled)
                                                    └──► DEAD_LETTER (exhausted)
CANCELLED (via API at any time)
```

**Hot path:** When a worker is already waiting, `CreateTask` routes the task in-memory via a oneshot channel — no database poll needed.

**Cold path:** If no worker is available, the task stays `PENDING` in Postgres. The `TaskReader` picks it up using `SELECT ... FOR UPDATE SKIP LOCKED` when a worker becomes free.

## SDKs

### Rust

```rust
use valka_sdk::{ValkaWorker, TaskContext};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let worker = ValkaWorker::builder()
        .name("email-worker")
        .server_addr("http://[::1]:50051")
        .queues(&["emails"])
        .concurrency(8)
        .handler(|ctx: TaskContext| async move {
            let input: serde_json::Value = ctx.input()?;
            ctx.log(&format!("Processing: {}", ctx.task_name)).await;

            // ... do work ...

            Ok(serde_json::json!({"status": "delivered"}))
        })
        .build()
        .await?;

    worker.run().await
}
```

### TypeScript / JavaScript

```bash
npm install @valka/sdk
```

```typescript
import { ValkaClient } from "@valka/sdk";

const client = new ValkaClient("http://localhost:8080");

// Create a task
const task = await client.createTask({
  queue_name: "emails",
  task_name: "send-welcome",
  input: { to: "user@example.com", template: "welcome" },
});

// Subscribe to real-time events
client.subscribeEvents((event) => {
  console.log(`Task ${event.task_id}: ${event.previous_status} → ${event.new_status}`);
});
```

### REST API

Any language can interact with Valka through the REST API:

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/api/v1/tasks` | Create a task |
| `GET` | `/api/v1/tasks` | List tasks (filterable by queue, status) |
| `GET` | `/api/v1/tasks/:id` | Get task details |
| `POST` | `/api/v1/tasks/:id/cancel` | Cancel a task |
| `GET` | `/api/v1/tasks/:id/runs` | Get task execution history |
| `GET` | `/api/v1/tasks/:id/runs/:run_id/logs` | Get logs for a run |
| `GET` | `/api/v1/workers` | List connected workers |
| `GET` | `/api/v1/dead-letters` | List dead letter entries |
| `GET` | `/api/v1/events` | Subscribe to events (SSE) |
| `GET` | `/healthz` | Health check |
| `GET` | `/metrics` | Prometheus metrics |

## CLI

```bash
# Task management
valka task create --queue emails --name send-welcome --input '{"to": "user@example.com"}'
valka task get <task_id>
valka task list --queue emails --status RUNNING --limit 50
valka task cancel <task_id>

# Log tailing
valka logs tail <task_run_id>

# Start the server
valka server --config valka.toml
```

## Web Dashboard

Valka ships with a built-in React dashboard for monitoring and management.

```bash
cd web && npm install && npm run dev
```

**Pages:**
- **Dashboard** — Queue summaries, throughput stats, system health at a glance
- **Tasks** — Searchable task list with filters by queue, status, and time range
- **Task Detail** — Full task metadata, execution history, and real-time log viewer
- **Workers** — Connected workers with queue assignments and heartbeat status
- **Events** — Live event stream showing every task state transition
- **Dead Letters** — Inspect failed tasks that exhausted all retries

In production, the dashboard is bundled into the server binary and served automatically at the root path.

## Configuration

Valka uses layered configuration via [figment](https://github.com/SergioBenitez/Figment): defaults → `valka.toml` → environment variables.

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `VALKA_DATABASE_URL` | — | PostgreSQL connection string |
| `VALKA_GRPC_ADDR` | `[::1]:50051` | gRPC listen address |
| `VALKA_HTTP_ADDR` | `0.0.0.0:8080` | REST/HTTP listen address |
| `RUST_LOG` | `valka=info,tower_http=info` | Log level filter |

### Configuration File

```toml
# valka.toml
database_url = "postgres://valka:valka@localhost:5432/valka"
grpc_addr = "[::1]:50051"
http_addr = "0.0.0.0:8080"

[matching]
num_partitions = 16
max_buffer_per_partition = 1024

[scheduler]
reaper_interval_secs = 10
lease_timeout_secs = 300
retry_base_delay_secs = 1
retry_max_delay_secs = 3600
```

## Deployment

### Docker Compose (recommended)

```bash
docker compose up
```

This starts PostgreSQL 17 and the Valka server with the web dashboard. The server is available at:
- **REST + Dashboard:** `http://localhost:8080`
- **gRPC:** `localhost:50051`

### Docker Image

The multi-stage Dockerfile produces a minimal Debian-based image (~100MB) containing:
- `valka-server` binary
- `valka` CLI binary
- Pre-built web dashboard assets

```bash
docker build -t valka .
docker run -e VALKA_DATABASE_URL=postgres://... -p 8080:8080 -p 50051:50051 valka
```

### From Source

```bash
cargo build --release --workspace
./target/release/valka-server
```

## Project Structure

```
valka/
├── proto/valka/v1/         # Protobuf service definitions
├── crates/
│   ├── valka-proto          # Generated gRPC stubs
│   ├── valka-core           # Shared types, config, errors, metrics
│   ├── valka-db             # PostgreSQL pool, migrations, queries
│   ├── valka-matching       # In-memory matching engine + partition tree
│   ├── valka-dispatcher     # Worker stream management + task dispatch
│   ├── valka-scheduler      # Retry engine, lease reaper, DLQ, delayed tasks
│   ├── valka-cluster        # Gossip protocol + hash ring (Phase 2)
│   ├── valka-server         # Main binary: gRPC + REST + scheduler
│   ├── valka-sdk            # Rust worker SDK
│   ├── valka-cli            # CLI tool
│   └── valka-tests          # Integration test suite
├── examples/rs/             # Rust examples (producer, worker, full lifecycle)
├── sdks/typescript/         # TypeScript/JavaScript SDK
├── web/                     # React 19 dashboard (Vite + Tailwind)
├── docker-compose.yml       # PostgreSQL + server
└── Dockerfile               # Multi-stage production build
```

## Database

Valka uses PostgreSQL 17 with automatic migrations. Tables:

| Table | Purpose |
|-------|---------|
| `tasks` | Task definitions and current state |
| `task_runs` | Execution attempts with lease tracking |
| `task_logs` | Structured log entries per run |
| `dead_letter_queue` | Tasks that exhausted all retries |
| `workers` | Connected worker registry |

Key design choices:
- `SKIP LOCKED` for contention-free dequeuing across multiple server instances
- Advisory locks for leader election (scheduler runs on one node only)
- UUIDv7 primary keys for time-sorted, globally unique IDs
- JSONB columns for flexible task input, output, and metadata

## Roadmap

- [x] Core task queue with retry and DLQ
- [x] gRPC bidirectional streaming workers
- [x] REST API + CLI
- [x] Real-time event streaming (gRPC + SSE)
- [x] Structured log streaming
- [x] Web dashboard
- [x] Rust SDK
- [x] TypeScript SDK
- [ ] Multi-node clustering (chitchat gossip + consistent hashing)
- [ ] Task priorities and weighted fair queuing
- [ ] Cron/recurring task scheduling
- [ ] Rate limiting per queue
- [ ] Python SDK
- [ ] Go SDK

## License

Apache License 2.0 — see [LICENSE](LICENSE) for details.
