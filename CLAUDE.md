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
cargo test --workspace           # Run all tests (18 unit/integration tests)
cargo run -p valka-server        # Start the server (gRPC :50051, REST :8080)
cargo run -p valka-cli -- --help # CLI tool
cargo clippy --workspace         # Lint
cargo fmt --check                # Format check
```

## Workspace Structure (11 crates)

| Crate | Purpose |
|-------|---------|
| `valka-proto` | Generated gRPC stubs from proto files |
| `valka-core` | Shared types (TaskId, WorkerId, PartitionId), config (figment), errors, metrics |
| `valka-db` | PG pool, migrations, query modules (tasks, task_runs, task_logs, dead_letter) |
| `valka-matching` | In-memory matching service + partition tree + TaskReader (PG SKIP LOCKED) |
| `valka-dispatcher` | Worker gRPC stream management, heartbeat, task dispatch |
| `valka-scheduler` | PG advisory lock election, lease reaper, retry engine, DLQ, delayed promoter |
| `valka-cluster` | chitchat gossip + consistent hash ring (single-node in Phase 1) |
| `valka-server` | Binary: assembles all services (gRPC + REST + scheduler + log ingester) |
| `valka-sdk` | Worker SDK: ValkaClient (task CRUD) + ValkaWorker (builder pattern, stream) |
| `valka-cli` | CLI: `valka task create/get/list/cancel`, `valka logs tail` |
| `valka-tests` | Integration tests: matching, lifecycle, retry |

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

### Sync Match (Hot Path)
CreateTask → PG INSERT → MatchingService.offer_task() → oneshot to waiting worker → gRPC push
If no worker waiting, task stays PENDING for TaskReader (cold path via SKIP LOCKED).

## Configuration

Layered via figment: defaults → `valka.toml` → env vars (VALKA_ prefix).

Key env vars:
- `VALKA_DATABASE_URL` — PostgreSQL connection string
- `VALKA_GRPC_ADDR` — gRPC listen address (default `[::1]:50051`)
- `VALKA_HTTP_ADDR` — REST/HTTP listen address (default `0.0.0.0:8080`)
- `RUST_LOG` — tracing filter (default `valka=info,tower_http=info`)

## Database

PostgreSQL 17. Migrations at `crates/valka-db/migrations/`. Tables: tasks, task_runs, task_logs, dead_letter_queue, workers.

`sqlx` with runtime query checking (not compile-time). Use `sqlx::query!` only if `DATABASE_URL` is set.

## Tests

Tests are in `crates/valka-tests/src/`:
- `matching_tests.rs` — sync match, buffer, round robin, tree forwarding, deregister
- `lifecycle_tests.rs` — IDs, partitions, config, status roundtrip
- `retry_tests.rs` — exponential backoff, caps, SDK retry policy

No database required for these tests (they test in-memory logic).

## Coding Conventions

- Edition 2024, resolver "3", rust-version "1.85"
- `rustfmt.toml`: max_width=100, use_field_init_shorthand=true
- Error handling: `thiserror` for library errors, `anyhow` in binaries
- Async: all async code uses tokio runtime
- Allocator: jemalloc on Linux via `tikv-jemallocator`

## WebUI

Development:
```bash
cd web && npm install && npm run dev  # Vite dev server on :5173, proxies /api to :8080
```

Production: `npm run build` produces `web/dist/`, served by axum fallback.
