# Valka — Project Guide

## Overview

Valka is a polyglot distributed task queue built entirely in Rust. Only external dependency: an S3-compatible bucket (or a local directory in dev). No database, no NATS, no Redis, no RabbitMQ.

**Architecture (branch `wal`):** a write-ahead log on object storage is the only truth. Every mutation is validated against in-RAM shard state, applied, appended to a group-committed WAL segment, and acknowledged only once the segment is in the bucket. gRPC bidirectional streaming for worker communication, in-memory matching service with partition trees for task routing, tokio::sync::broadcast for event fan-out. Full spec: `docs/wal/DESIGN.md`, plan: `docs/wal/PLAN.md`.

## Quick Start

```bash
# Run the server against ./data (local backend, no services needed)
cargo run -p valka-server

# Or against MinIO: docker compose up -d minio minio-init, then set VALKA_STORAGE__* (see README)

# In another terminal, run the example worker
cargo run -p valka-examples --example worker

# In another terminal, create tasks
cargo run -p valka-examples --example producer
```

## Build Commands

```bash
cargo build --workspace          # Build all crates
cargo test --workspace           # All tests (in-memory object store, no services needed)
cargo run -p valka-server        # Start the server (gRPC :50051, REST :8989)
cargo run -p valka-cli -- --help # CLI tool
cargo clippy --workspace         # Lint
cargo fmt --check                # Format check
```

## Workspace Structure (12 members)

| Crate | Purpose |
|-------|---------|
| `valka-proto` | Generated gRPC stubs from proto files |
| `valka-core` | Shared types (TaskId, WorkerId, ShardId), config (figment), errors, metrics, shard helpers |
| `valka-wal` | Object-store WAL: `Store` wrapper (memory/local/S3 + CAS), segment codec, group-commit `WalWriter`, reader/replay, snapshots, ownership (assignment CAS), log chunks, `FaultyStore` for tests |
| `valka-engine` | 4096 in-RAM shard state machines, command API (`Engine`), timer wheel (leases/retries/delayed), pending index + feeder, snapshotter + WAL truncation, recovery |
| `valka-matching` | In-memory matching service + partition tree + `MatchingSink` (engine → matching adapter) |
| `valka-dispatcher` | Worker gRPC stream management, heartbeat, task dispatch, signal delivery |
| `valka-cluster` | chitchat gossip + consistent hash ring + node forwarder (multi-node ownership: phase 2) |
| `valka-server` | Binary: assembles all services (gRPC + REST + engine + log ingester) |
| `valka-sdk` | Rust worker SDK: ValkaClient (task CRUD) + ValkaWorker (builder pattern, stream) |
| `valka-cli` | CLI: `valka task create/get/list/cancel`, `valka logs tail` |
| `valka-tests` | Unit + integration test suite (244 tests, plus MinIO-gated) |
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

### Engine mutation pattern
Every write goes through `Engine::mutate`: lock the shard, validate against RAM, build a `WalRecord`, `ShardState::apply` it, `WalWriter::append`, unlock, then `await durable` before acknowledging. Things that must only happen once durable (events, offering to matching) go in `after_durable`. `ShardState::apply` is total: a record whose precondition no longer holds is a no-op.

### Snapshot exactness
Envelopes carry a per-shard `shard_seq`; snapshots store the last applied `shard_seq`. Replay skips records at or below it, so snapshots never rely on idempotency. Segments below every dirty shard's `dirty_since_lsn` (and below the durable LSN) are truncated after a snapshot round.

### Time in tests
`TokioClock` derives wall time from tokio's instant, so `#[tokio::test(start_paused = true)]` + `tokio::time::advance` moves leases, retries and timestamps deterministically. `FaultyStore` (valka-wal) injects latency, errors, lost PUT acks and freezes.

### DashMap Guard Safety
- DashMap read/write guards can deadlock if you hold one while acquiring another
- Pattern: use block scoping `let result = { dashmap.get_mut(key)... };` to drop guard before proceeding
- Critical in `sync_match.rs`: drop partition guard BEFORE calling `try_forward_up`

### UUIDv7
- All IDs (TaskId, WorkerId, etc.) are UUIDv7 — time-sortable
- Generated app-side: `uuid::Uuid::now_v7().to_string()`

### Task Lifecycle
```
PENDING → RUNNING → COMPLETED
                  → FAILED (non-retryable, attempts < max_retries)
                  → RETRY → PENDING (promoted by the engine timer at next_attempt_at)
                  → DEAD_LETTER (attempts >= max_retries)
any non-terminal → CANCELLED
```
Every arrow is a `WalRecord` (`TaskCreated`, `TaskDispatched`, `RunCompleted`, `RunFailed`, `LeaseExpired`, `TaskPromoted`, `TaskCancelled`, ...). `DISPATCHING` still exists in the proto enum but is no longer a stored state.

### Task Signals
Workers can receive signals on running tasks (e.g. progress requests, config updates). Signals flow through the dispatcher over the existing gRPC bidi stream:
- `POST /api/v1/tasks/:id/signal` or gRPC `SendSignal` creates a signal
- Dispatcher delivers `TaskSignal` to the worker; worker replies with `SignalAck`
- Status tracking: PENDING → DELIVERED → ACKNOWLEDGED
- On worker disconnect, unacknowledged signals reset to PENDING for redelivery

### Sync Match (Hot Path)
CreateTask → WAL append → durable → `MatchingSink::offer` → oneshot to waiting worker → gRPC push
If matching buffers are full, the task stays in the engine's RAM pending index; the feeder loop tops matching up as capacity frees. Dispatch records (`TaskDispatched`) are written asynchronously by design (at-least-once).

## Configuration

Layered via figment: defaults → `valka.toml` → env vars (VALKA_ prefix).

Key env vars:
- `VALKA_STORAGE__BACKEND` — `local` (default, `./data`), `s3`, or `memory`
- `VALKA_STORAGE__BUCKET` / `VALKA_STORAGE__ENDPOINT` / `VALKA_STORAGE__ALLOW_HTTP` — S3 settings (MinIO: `http://localhost:9000`, allow_http)
- `VALKA_NODE_ID` — set a stable id in production; a node replays `wal/<node_id>/`
- `VALKA_WAL__FLUSH_INTERVAL_MS` — group-commit window (default 50)
- `VALKA_GRPC_ADDR` — gRPC listen address (default `0.0.0.0:50051`)
- `VALKA_HTTP_ADDR` — REST/HTTP listen address (default `0.0.0.0:8989`)
- `RUST_LOG` — tracing filter (default `valka=info,tower_http=info`)

## Storage layout (bucket)

```
assignment                          shard → {node, epoch}, CAS'd
wal/{node_id}/{epoch:08}-{seq:016}.seg   WAL segments (header + CRC-framed zstd JSON records)
snapshots/{shard:04}/{epoch}-{seq}.snap  per-shard snapshots (zstd JSON)
logs/{run_id}/{uuidv7}.log          task log chunks
```
Dev default is `./data` via the `local` backend. MinIO for S3 semantics: `docker compose up -d minio minio-init`.

## Tests

```bash
cargo test --workspace                                   # everything, no external services
cargo test -p valka-wal                                  # 21: store, codec, writer under faults, snapshots, ownership
cargo test -p valka-engine                               # 22: lifecycle, timers (paused time), recovery, crash/replay proptest
cargo test -p valka-tests                                # 244: unit + REST + lifecycle + dispatcher + gRPC e2e with the SDK
VALKA_TEST_S3_ENDPOINT=http://localhost:9000 AWS_ACCESS_KEY_ID=minioadmin AWS_SECRET_ACCESS_KEY=minioadmin \
  cargo test -p valka-tests --features minio minio_    # real S3 conditional writes (bucket valka-test)
```

Integration tests build a `TestNode` (engine + dispatcher + REST router) on an in-memory object store per test; `Store::wrap` over a shared `InMemory` simulates a node crash + restart on the same bucket.

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
