# WAL rewrite — implementation plan

## Phase 1 — single node (this branch, now)

- [x] `valka-core`: drop sqlx; `StorageConfig` (backend, bucket/path, flush knobs); shard helpers; `ServerError::Storage`.
- [x] `valka-wal` (new): `Store` wrapper over `object_store` + `FaultyStore`; `WalRecord`; segment codec; `WalWriter` (group commit, parked acks, pipelining); `WalReader`; snapshot codec; `Ownership` (assignment CAS, single-node path).
- [x] `valka-engine` (new): `ShardState` + apply; `Engine` command API (create/get/list/cancel/delete/clear/signal/dispatch/complete/fail/heartbeat); timer wheel; recovery; snapshotter; log store.
- [x] `valka-dispatcher`: replace PG calls with `Engine`.
- [x] `valka-matching`: `TaskReader` → in-RAM feeder from `Engine` pending heaps.
- [x] `valka-scheduler`: delete (reaper/retry/DLQ/promoter live in engine timers).
- [x] `valka-server`: rewire gRPC/REST/internal; log ingester → log store; remove migrations.
- [x] `valka-db`: delete.
- [x] Tests: rewrite unit tests; replace PG integration suites with in-memory engine + REST suites; crash/replay proptests; MinIO feature.
- [x] docker-compose: MinIO instead of Postgres; CI update; docs.

Phase 1 test inventory: valka-wal 21, valka-engine 22 (incl. crash/replay property test
under injected faults), valka-tests 244 (unit + REST + lifecycle + dispatcher + real gRPC
end-to-end with the Rust SDK, incl. server crash/restart), MinIO-gated 2.

## Cluster UI (phase 1 half) — shipped

- [x] Cluster read API: node stats, shard map, storage stats, snapshot trigger.
- [x] Dashboard: Cluster / Node / Shards (4096-cell heatmap) / Storage pages, cluster strip,
      Node columns, Shard row. See `CLUSTER_UI.md` for the phase 2 half.

## Step checkpoints — shipped

- [x] `TaskCheckpointed` record, fenced to the current run; checkpoints snapshotted with the task
      and carried on every `TaskAssignment`. `WorkerService.Checkpoint` RPC,
      `GET /api/v1/tasks/{id}/checkpoints`, Steps table on the task page. DESIGN.md §16.
- [x] SDKs: `step`/`checkpoint` in Rust, TypeScript, Go and Python; `steps` example per language.

Test inventory now: valka-wal 25, valka-engine 42, valka-tests 266, web 22, Go SDK 9, Python SDK 12.

## Phase 2 — cluster

Design, decisions and milestones: [`PHASE2.md`](PHASE2.md).

- [x] M0 Phase-1 hardening: snapshot durability fix, `AlreadyExists` read-back, end-to-end result acks in all four SDKs, non-blocking SDK receive loops, worker drain and server-restart protocol, exit on poison, log budget.
- [x] M1 TLA+ model of leases, ownership, sealing, splitting and GC (`spec/tla/`); TLC in CI (`tla` job: five passing configurations, six counterexamples that must keep failing). Found: a split must read from a lease-held truncation watermark (not the lowest listed position, nor the max of both), load owner records and bases only after reading, rebuild only owner-named generations, and re-check its marker before writing; "no snapshot ⇒ empty" at any gen; leases are never deleted (PHASE2 §19).
- [x] M2 Formats: `cluster.json`, 16-bit shard ids, `gen` in records, sealable segments (v2 header with node id, flags, shard list), `owners/`, `nodes/`, `snapshots/{hh}/{shard}/{gen}-{seq}-{ptr}`, `splits/`, hashed prefixes, storage self-test, clustered mode refuses local/memory.
- [x] M3 One node on the full protocol: incarnations (lease CAS at start), renewal + self-fencing, own-lease check after every segment PUT, per-shard phases (Unowned → Loading → Serving → Releasing), claim with acquisition snapshot, release with final snapshot, split; a restart splits its own previous incarnations and claims. Truncation raises the lease's watermark before deleting (fixes the LIST-hole hazard, see PHASE2 §8.3).
- [x] M4 Membership from the bucket (LIST `nodes/` + ETag ageing by the observer's own clock), guardian tombstones and splits, rendezvous placement with join delay, residency and handover limits, GC (split logs, snapshots, unreferenced markers; never leases). Multi-engine tests: convergence, kill, freeze-then-wake (fenced), bucket cut-off (self-fence before takeover), drain, crashed claim, restart, GC; MinIO: self-test and two-node takeover. Gossip is not needed for correctness and is not wired in yet (an accelerator for suspicion, M8); the turmoil harness moves to M9.
- [x] M5 Queue partitions (`queues/{hh}/{queue}`, jump hash for keyed creates, local-first otherwise, exact dedupe across a resize) and request routing: every REST/gRPC call goes through `valka-server::api::Api` (local, or forwarded to the shard owner over `InternalService.Execute` with NOT_OWNER hints and owner-record fallback); lists/counts/workers/stats/shard map are scatter-gathered with an `X-Valka-Partial` header. Internal listener on `cluster.internal_addr` with the cluster token (constant-time) and optional mTLS. Gossip ring, partitions and `ForwardTask` removed. Tests: 7 multi-node integration tests over real localhost gRPC (REST through any of 3 nodes, cross-node idempotency, resize dedupe, bad token, takeover, mTLS, gRPC API).
- [x] M6 Cross-node matching, pull-based: idle workers' nodes long-poll the owners of a queue's partitions (`Fetch`, a RAM-only reservation that lapses after 30 s); the dispatch is recorded by the owner for a specific worker and node (`Dispatch`, idempotent on a caller-minted run id); a run that never reached its worker goes back without spending an attempt (`RunReturned`). Results, heartbeats (batched per owner), checkpoints and signal acks go to the owner through the dispatcher's `TaskRouter`; cancels and signal deliveries go to the run's node; subscribers see every node's events (`InternalService.Events` relay). Matching offers only to waiting workers, so runnable tasks stay in the owner's index for local and remote takers alike. Tests: SDK worker on one node running tasks of three nodes with steps, signals/acks/heartbeats/results/cancels across nodes, cluster-wide events, a result applied after its owner died.
- [x] M7 Worker reattach: `WorkerHello.running` (task, run, attempt) sent by all four SDKs on every connect; the owner reattaches a current run to the worker's node (`RunReattached`), adopts a run whose dispatch record was lost (task PENDING at exactly that attempt, recorded under the worker's run id), or answers stale with a `TaskCancellation`; claimed shards hold recovered PENDING tasks for `reconcile_grace` so reconnecting workers win. Tests: engine outcomes, dispatcher reconnect handling, an e2e where a server crash loses the dispatch record and the reconnecting SDK worker's run is adopted (verified to run twice without the hello list), SDK unit tests in Go and Python.
- [x] M8 Operations: admin API behind `VALKA_ADMIN_TOKEN` with an audit record per action (`admin/…`): drain a node (it hands every shard over and exits cleanly), move a shard (operator pins in `placement.json`, honoured while the pinned node is eligible, bypassing min residency), unpin, set a queue's partition count (broadcast to every node), list queues and placement; version gate (a node refuses to join a cluster whose live nodes speak a newer protocol; effective protocol = min over live nodes); metrics for leases, fencing, claims, releases, tombstones, splits, forwarding, remote fetches, reattaches and WAL segments; Prometheus alert rules (`deploy/prometheus/alerts.yml`) and runbooks (`RUNBOOKS.md`). SIGTERM drain, min residency and handover limits landed in M4.
- [x] M9 (except the soak) Randomized multi-node simulation with a history checker (`valka-engine/src/sim_tests.rs`: kills, freezes, drains, restarts, lost PUT acks and bucket errors; 400 seeds clean; found three claim bugs, PHASE2 §19a); TLC `Restart` and `TwoNodes` simulated in CI and exhaustive nightly (`tla-nightly.yml`); chaos rig on real processes (`docker-compose.cluster.yml`, `scripts/chaos.sh`, `examples/rs/chaos.rs`: random `kill -9`, freezes and restarts under load, checks no acknowledged task is lost or unfinished and no idempotency key duplicated). Still open: network-split and bucket-slowdown chaos (toxiproxy), the 24 h soak.
- [x] M10 (compose and docs) Three-node `docker-compose.cluster.yml`; readable architecture guide (`docs/ARCHITECTURE.md`); DESIGN.md and PHASE2.md brought up to date with what was built; runbooks; docs site clustering and deployment pages; dashboard compatible with the cluster API (incarnations, generations, any shard count, membership). Out of the current scope: Helm chart, systemd units.

Independent of phase 2: payload blob externalisation (`blobs/…`) above a size threshold;
`ack=fast` per-task mode.
