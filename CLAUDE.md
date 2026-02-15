# Valka — Project Guide

## Overview

Valka is a polyglot distributed task queue built entirely in Rust. Only external dependency: PostgreSQL. No NATS, no Redis, no RabbitMQ.

**Architecture:** gRPC bidirectional streaming for worker communication, in-memory matching service with partition trees for task routing, PG SKIP LOCKED for durable dequeuing, tokio::sync::broadcast for event fan-out.

## Quick Start

```bash
# Start PostgreSQL
docker compose up -d postgres

# Run the server (applies migrations automatically)
cargo run -p valka-server

# In another terminal, run the example worker
cargo run -p valka-examples --example worker

# In another terminal, create tasks
cargo run -p valka-examples --example producer
```

## Build Commands

```bash
cargo build --workspace          # Build all crates
cargo test -p valka-tests        # Run unit tests (133 tests, no DB needed)
cargo run -p valka-server        # Start the server (gRPC :50051, REST :8989)
cargo run -p valka-cli -- --help # CLI tool
cargo clippy --workspace         # Lint
cargo fmt --check                # Format check
```

## Workspace Structure (12 members)

| Crate | Purpose |
|-------|---------|
| `valka-proto` | Generated gRPC stubs from proto files |
| `valka-core` | Shared types (TaskId, WorkerId, PartitionId), config (figment), errors, metrics |
| `valka-db` | PG pool, migrations, query modules (tasks, task_runs, task_logs, dead_letter, signals) |
| `valka-matching` | In-memory matching service + partition tree + TaskReader (PG SKIP LOCKED) |
| `valka-dispatcher` | Worker gRPC stream management, heartbeat, task dispatch, signal delivery |
| `valka-scheduler` | PG advisory lock election, lease reaper, retry engine, DLQ, delayed promoter |
| `valka-cluster` | chitchat gossip + consistent hash ring + node forwarder with circuit breaker |
| `valka-server` | Binary: assembles all services (gRPC + REST + scheduler + log ingester) |
| `valka-sdk` | Rust worker SDK: ValkaClient (task CRUD) + ValkaWorker (builder pattern, stream) |
| `valka-cli` | CLI: `valka task create/get/list/cancel`, `valka logs tail` |
| `valka-tests` | Unit + integration test suite (271 tests) |
| `examples/rs` | Rust examples (producer, worker, full_lifecycle) |

## SDKs

In addition to the Rust SDK (`valka-sdk` crate), polyglot SDKs live in `sdks/`:

| SDK | Location | Package |
|-----|----------|---------|
| **Rust** | `crates/valka-sdk` | `valka-sdk` (crate) |
| **TypeScript** | `sdks/typescript/` | `@valka/sdk` (npm) |
| **Go** | `sdks/go/` | `github.com/valka-queue/valka/sdks/go` |
| **Python** | `sdks/python/` | `valka` (PyPI) |

Examples for each language are in `examples/{rs,typescript,python,go}/`.

## Key Technical Patterns

### tonic 0.14 + prost
- Use `tonic_prost_build::configure()` in build.rs (NOT `tonic_build::configure()`)
- Runtime needs `tonic-prost` crate for `ProstCodec`
- Proto files at `proto/valka/v1/` (common, api, worker, events, internal)

### DashMap Guard Safety
- DashMap read/write guards can deadlock if you hold one while acquiring another
- Pattern: use block scoping `let result = { dashmap.get_mut(key)... };` to drop guard before proceeding
- Critical in `sync_match.rs`: drop partition guard BEFORE calling `try_forward_up`

### UUIDv7
- All IDs (TaskId, WorkerId, etc.) are UUIDv7 — time-sortable
- Generated app-side: `uuid::Uuid::now_v7().to_string()`

### Task Lifecycle
```
PENDING → DISPATCHING → RUNNING → COMPLETED
                                → FAILED
                     → RETRY → (back to PENDING via scheduler)
                     → DEAD_LETTER (max retries exceeded)
                     → CANCELLED
```

### Task Signals
Workers can receive signals on running tasks (e.g. progress requests, config updates). Signals flow through the dispatcher over the existing gRPC bidi stream:
- `POST /api/v1/tasks/:id/signal` or gRPC `SendSignal` creates a signal
- Dispatcher delivers `TaskSignal` to the worker; worker replies with `SignalAck`
- Status tracking: PENDING → DELIVERED → ACKNOWLEDGED
- On worker disconnect, unacknowledged signals reset to PENDING for redelivery

### Sync Match (Hot Path)
CreateTask → PG INSERT → MatchingService.offer_task() → oneshot to waiting worker → gRPC push
If no worker waiting, task stays PENDING for TaskReader (cold path via SKIP LOCKED).

## Configuration

Layered via figment: defaults → `valka.toml` → env vars (VALKA_ prefix).

Key env vars:
- `VALKA_DATABASE_URL` — PostgreSQL connection string
- `VALKA_GRPC_ADDR` — gRPC listen address (default `0.0.0.0:50051`)
- `VALKA_HTTP_ADDR` — REST/HTTP listen address (default `0.0.0.0:8989`)
- `RUST_LOG` — tracing filter (default `valka=info,tower_http=info`)

## Database

PostgreSQL 17. Migrations at `crates/valka-db/migrations/`. Tables: tasks, task_runs, task_logs, dead_letter_queue, workers, task_signals.

`sqlx` with runtime query checking (not compile-time). Use `sqlx::query!` only if `DATABASE_URL` is set.

## Tests

271 total tests across unit and integration suites.

### Unit Tests (133 tests, no DB needed)

```bash
cargo test -p valka-tests
```

Modules: cluster, config, dispatcher, error, heartbeat, lifecycle, matching, proto, retry, sdk.

### Integration Tests (138 tests, requires PostgreSQL)

```bash
DATABASE_URL=postgresql://valka:valka@localhost:5454/valka \
  cargo test -p valka-tests --features integration
```

Feature-gated with `#[cfg(all(test, feature = "integration"))]`. Uses `#[sqlx::test]` for per-test isolated temp databases.

Modules: db_tasks (22), db_task_runs (14), db_task_logs (6), db_dead_letter (6), db_signals (16), rest_api (37), scheduler (14), dispatcher (10), lifecycle (12).

## Coding Conventions

- Edition 2024, resolver "3", rust-version "1.88"
- `rustfmt.toml`: max_width=100, use_field_init_shorthand=true
- Error handling: `thiserror` for library errors, `anyhow` in binaries
- Async: all async code uses tokio runtime
- Allocator: jemalloc on Linux via `tikv-jemallocator`

## WebUI

Development:
```bash
cd web && npm install && npm run dev  # Vite dev server on :5173, proxies /api to :8989
```

Production: `npm run build` produces `web/dist/`, served by axum fallback.

Stack: React 19, TypeScript, Vite, Tailwind CSS, Radix UI, TanStack React Query.

Pages: Dashboard, Tasks, Task Detail (with runs, logs, signals tabs), Workers, Events, Dead Letters.
