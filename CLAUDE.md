# Valka — Project Guide

## Overview

Valka is a polyglot distributed task queue built entirely in Rust. Only external dependency: an S3-compatible bucket (or a local directory in dev). No database, no NATS, no Redis, no RabbitMQ.

**Architecture (branch `wal`):** a write-ahead log on object storage is the only truth. Every mutation is validated against in-RAM shard state, applied, appended to a group-committed WAL segment, and acknowledged only once the segment is in the bucket. Any number of nodes share one bucket: each shard has one owner (owner records moved by CAS, node leases, an own-lease check after every segment PUT), any node takes any request and forwards it to the owner, and workers on any node run tasks owned by any node. gRPC bidirectional streaming for worker communication, in-memory matching service, tokio::sync::broadcast for event fan-out. Start here: `docs/ARCHITECTURE.md` (readable overview). Specs: `docs/wal/DESIGN.md` (one node), `docs/wal/PHASE2.md` (many nodes); plan: `docs/wal/PLAN.md`; operations: `docs/wal/RUNBOOKS.md`.

## Quick Start

```bash
# Run the server against ./data (local backend, no services needed)
cargo run -p valka-server

# Or against MinIO: docker compose up -d minio minio-init, then set VALKA_STORAGE__* (see README)

# Or a 3-node cluster (REST :8981-8983, gRPC :50061-50063): docker compose -f docker-compose.cluster.yml up -d --build
# Chaos test against it: scripts/chaos.sh (DURATION=seconds, NO_BUILD=1)

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
| `valka-cluster` | Node-to-node layer: internal RPC commands/queries (`rpc.rs`), peer clients with circuit breaker (`peer.rs`), cluster token + mTLS (`security.rs`) |
| `valka-server` | Binary: assembles all services (gRPC + REST + internal listener + engine + log ingester); `api.rs` routes every request (local / forward / scatter), `feeder.rs` remote fetching, `relay.rs` cluster events, `admin.rs` admin API |
| `valka-sdk` | Rust worker SDK: ValkaClient (task CRUD) + ValkaWorker (builder pattern, stream) |
| `valka-cli` | CLI: `valka task create/get/list/cancel`, `valka logs tail`, `valka worker list`, `valka cluster status/members/drain/move/unpin`, `valka queue list/partitions` (`--server` repeatable, `--api-key`, `--admin-token`) |
| `valka-tests` | Unit + integration test suite (255 tests, incl. 3-node clusters over real localhost gRPC, plus MinIO-gated) |
| `examples/rs` | Rust examples (producer, worker, full_lifecycle, signal_demo, steps) |

## SDKs

In addition to the Rust SDK (`valka-sdk` crate), polyglot SDKs live in `sdks/`:

| SDK | Location | Package |
|-----|----------|---------|
| **Rust** | `crates/valka-sdk` | `valka-sdk` (crate) |
| **TypeScript** | `sdks/typescript/` | `@valka/sdk` (npm) |
| **Go** | `sdks/go/` | `github.com/IWhitebird/Valka/sdks/go` |
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
Envelopes carry a per-shard `shard_seq` and the shard's ownership generation `gen`; snapshots store the last applied `shard_seq`. Replay skips records at or below it and records of other generations, so snapshots never rely on idempotency. After a snapshot round the node raises its lease's `trunc` watermark, then deletes its own segments below every dirty shard's `dirty_since_lsn` (and below the durable LSN), ascending.

### Ownership (docs/wal/PHASE2.md)
One code path for one node or many. Start = lease CAS (`inc + 1`, fences the previous run of the node id) → split the node's own unsplit incarnations → placement claims shards. Claim: owner record CAS to `gen + 1` (allowed if missing, released, or its incarnation is dead *and* split) → load newest snapshot → acquisition snapshot pointing into my log → Serving. Release: Releasing → sync → final snapshot → CAS released. A guardian tombstones a lease unchanged for `lease_timeout` (its own clock) and splits the dead log (`valka-engine/src/ownership.rs`, `cluster.rs`). Every segment PUT is followed by a conditional GET of the node's own lease before acks are released. Membership is a LIST of `nodes/` every 3 s (ETag ageing by the observer's own clock); no gossip. Placement is rendezvous hashing over eligible nodes plus operator pins (`placement.json`). A claim that may have passed its CAS but failed is "stranded" and retried by its claimer. Multi-node tests: `valka-engine/src/cluster_tests.rs`, `safety_tests.rs` (TLA+ findings), `sim_tests.rs` (randomized, history checker; `VALKA_SIM_SEEDS=N`). TLA+ model: `spec/tla/`.

### Requests and workers across nodes (PHASE2 §9–§11)
Queues have partitions (`queues/{hh}/{queue}`, default 16); keyed creates use `jump_hash(key, P)`, unkeyed prefer a local partition. Every REST/gRPC call goes through `valka_server::api::Api`: id-routed commands run on the shard owner (`InternalService.Execute`, JSON `valka_cluster::Command`, NOT_OWNER hints, owner-record fallback); lists/stats are scattered (`X-Valka-Partial`). Workers: idle workers' nodes long-poll owners (`Command::Fetch`, RAM reservation, 30 s TTL), dispatch is recorded by the owner (`Command::Dispatch`, idempotent on the run id), an undelivered run goes back with `RunReturned` (no attempt spent). The dispatcher's `TaskRouter` sends results/heartbeats/checkpoints/signal acks to owners; cancels and signals go to the run's node. Reconnecting workers send `WorkerHello.running`: Reattached / Adopted / Stale. Internal calls need `VALKA_CLUSTER__TOKEN` (constant-time), mTLS optional.

### Public API keys, limits and safe retries
- `VALKA_API_KEYS` (comma-separated) guards REST `/api/v1/*` (not the admin routes, which use `VALKA_ADMIN_TOKEN`) and gRPC `ApiService`/`WorkerService` (`valka_server::auth`: REST middleware + tonic interceptor, constant-time). Unset = open with a startup warning. `/healthz`, `/metrics`, the dashboard files and gRPC health/reflection stay open; the SSE stream also takes `?access_token=`. A 401 for a bad API key has code `UNAUTHENTICATED`; the admin API's is `UNAUTHORIZED`.
- `[limits]` (`LimitsConfig`): `max_tasks_in_ram` (default 1M; creates over it get `ServerError::ResourceExhausted` → 429 / RESOURCE_EXHAUSTED; near it finished tasks are evicted early, keeping a minute for retried creates) and `max_payload_bytes` (input/metadata, default 1 MiB). The engine samples tasks in RAM every second (`valka_tasks_in_ram`).
- A create with an idempotency key already in use returns the original task (REST 200 instead of 201, `idempotent-replayed: true`; gRPC response metadata), after a WAL sync so the answer is durable. All SDK clients generate `auto-…` keys for keyless creates, retry transient failures (unavailable, timeout, 429/502/503/504) on the next node of their address list, and never retry cancels or signals; workers take several addresses and rotate on reconnect. Keys: `VALKA_API_KEY` env in every SDK and the CLI.

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

### Step Checkpoints
A running run records completed steps via the unary `WorkerService.Checkpoint` RPC → `TaskCheckpointed` record (acked once durable). Only the task's current RUNNING run may checkpoint; `apply` re-checks this. Checkpoints live on `TaskState.checkpoints` (snapshotted with the task) and ride on every `TaskAssignment`, so SDK `step(name, fn)` skips steps a previous attempt finished. Limits in `valka_engine::MAX_*`. Spec: `docs/wal/DESIGN.md` §16. REST: `GET /api/v1/tasks/:id/checkpoints`.

### Result delivery and drain
Every `TaskResult` gets a `ResultAck` (APPLIED / STALE / RETRY) from `Engine::report_result`, which is idempotent and syncs the WAL before confirming an already-ended run. SDKs keep results until APPLIED or STALE, resend on reconnect, and heartbeat the task meanwhile. `GracefulShutdown` stops dispatch but keeps the stream for results; on server SIGTERM each worker stream gets `ServerShutdown` and workers reconnect. A poisoned WAL writer makes the server exit non-zero (`valka_server::server::wait_for_exit`). DESIGN.md §17.

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
- `VALKA_NODE_ID` — stable node id (default: the host name); a restarted node splits its previous incarnation's log at start, so its shards are claimable at once (alone it reclaims them; in a cluster peers take them and placement moves them back after `join_delay` + `min_residency`)
- `VALKA_CLUSTER__NUM_SHARDS` — shard count of a new bucket (default 4096; tests use 256)
- `VALKA_CLUSTER__ENABLED` / `VALKA_CLUSTER__TOKEN` — allow several nodes on one bucket (S3 only)
- `VALKA_CLUSTER__INTERNAL_ADDR` (default `0.0.0.0:50052`) / `VALKA_CLUSTER__ADVERTISE_{INTERNAL,GRPC,HTTP}_ADDR` — node-to-node listener and the addresses peers/clients use
- `VALKA_CLUSTER__TLS_{CERT,KEY,CA}` — optional mTLS for internal traffic
- `VALKA_CLUSTER__LEASE_TIMEOUT_MS` (10000), `JOIN_DELAY_MS` (15000), `MIN_RESIDENCY_SECS` (300), ... — see `docs/ARCHITECTURE.md` §12
- `VALKA_ADMIN_TOKEN` — enables the admin API (drain, move/pin shard, partitions, audit) behind `Authorization: Bearer`
- `VALKA_API_KEYS` — comma-separated keys for the public API (unset = open, with a warning); clients use `VALKA_API_KEY`
- `VALKA_LIMITS__MAX_TASKS_IN_RAM` (1000000) / `VALKA_LIMITS__MAX_PAYLOAD_BYTES` (1048576) — per-node backpressure and payload size
- `VALKA_WAL__FLUSH_INTERVAL_MS` — group-commit window (default 50)
- `VALKA_GRPC_ADDR` — gRPC listen address (default `0.0.0.0:50051`)
- `VALKA_HTTP_ADDR` — REST/HTTP listen address (default `0.0.0.0:8989`)
- `RUST_LOG` — tracing filter (default `valka=info,tower_http=info`)

## Storage layout (bucket)

```
cluster.json                                   cluster id, num_shards, format (created once)
nodes/{node}/lease                             inc, state, counter, trunc watermark, addrs; CAS'd
owners/{hh}/{shard}                            {node, inc, gen, owned|released}; CAS'd on moves
wal/{node}/{inc:010}-{seq:016}.seg             WAL segments (v2 header + CRC-framed zstd JSON), create-only
snapshots/{hh}/{shard}/{gen}-{seq}-{ptr}.snap  per-shard snapshots; ptr = node.inc.seq | final | split
splits/{node}/{inc}                            "this dead incarnation's log is split"
logs/{hh}/{run_id}/{uuidv7}.log                task log chunks
queues/{hh}/{queue}                            partition count; CAS'd by admin changes
placement.json                                 operator shard pins
admin/{ts}-{id}.json                           admin audit records
```
`object_store` lists by path segment: LIST prefixes must end at a `/`.
Dev default is `./data` via the `local` backend. MinIO for S3 semantics: `docker compose up -d minio minio-init`.

## Tests

```bash
cargo test --workspace                                   # everything, no external services
cargo test -p valka-wal                                  # 40: store, codec, writer under faults, snapshots, leases, owners, split/seal
cargo test -p valka-engine                               # 64: lifecycle, timers (paused time), recovery, checkpoints, results, log budget, crash/replay proptest, multi-node takeover/drain/fencing, TLA+ regressions, randomized simulation
cargo test -p valka-tests                                # 255: unit + REST + lifecycle + dispatcher + gRPC e2e with the SDK, server exit, 3-node routing/workers/admin/mTLS
VALKA_SIM_SEEDS=200 cargo test -p valka-engine randomized_cluster   # more simulation seeds
scripts/chaos.sh                                         # chaos on the 3-node compose cluster (docker)
VALKA_TEST_S3_ENDPOINT=http://localhost:9000 AWS_ACCESS_KEY_ID=minioadmin AWS_SECRET_ACCESS_KEY=minioadmin \
  cargo test -p valka-tests --features minio minio_    # real S3: CAS, self-test, two-node takeover (bucket valka-test)
```

Integration tests build a `TestNode` (engine + dispatcher + REST router) on an in-memory object store per test; `Store::wrap` over a shared `InMemory` simulates a node crash + restart on the same bucket.

## Coding Conventions

- Edition 2024, resolver "3", rust-version "1.89"
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

Pages: Dashboard, Tasks, Task Detail (with runs, logs, signals, steps), Workers, Cluster (overview + membership, node detail, shard heatmap for any shard count, storage, Operations: drain/move/unpin/partitions/audit behind the admin token), Events, Dead Letters. When the server has API keys, a 401 `UNAUTHENTICATED` shows a sign-in screen (`src/components/auth`); the key lives in localStorage, the admin token in sessionStorage (`src/lib/credentials.ts`).

Brand: logo, icons, banners and colors are generated by `python3 assets/brand/build.py` (which also installs the web icons into `web/public/`, `www/app/`, `www/public/`); see `assets/brand/README.md`. Never hand-edit those files.

Web tests: `cd web && npm test` (vitest + Testing Library; helpers in `src/lib`, components in `src/components/{cluster,task-detail}`).

SDK tests: `cd sdks/go && go test ./...`; `cd sdks/python && PYTHONPATH=src python -m pytest tests && ruff check src tests` (needs `httpx`, `grpcio`, `protobuf`, `pytest`, `ruff`); TypeScript: `cd sdks/typescript && npm run typecheck && npm test` (builds, then `node --test`). Web: `cd web && npm run lint && npm test`.
