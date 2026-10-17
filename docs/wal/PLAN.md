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
- [ ] M1 TLA+ model of leases, ownership, sealing, splitting and GC; TLC in CI.
- [x] M2 Formats: `cluster.json`, 16-bit shard ids, `gen` in records, sealable segments (v2 header with node id, flags, shard list), `owners/`, `nodes/`, `snapshots/{hh}/{shard}/{gen}-{seq}-{ptr}`, `splits/`, hashed prefixes, storage self-test, clustered mode refuses local/memory.
- [x] M3 One node on the full protocol: incarnations (lease CAS at start), renewal + self-fencing, own-lease check after every segment PUT, per-shard phases (Unowned → Loading → Serving → Releasing), claim with acquisition snapshot, release with final snapshot, split; a restart splits its own previous incarnations and claims. Truncation raises the lease's watermark before deleting (fixes the LIST-hole hazard, see PHASE2 §8.3).
- [x] M4 Membership from the bucket (LIST `nodes/` + ETag ageing by the observer's own clock), guardian tombstones and splits, rendezvous placement with join delay, residency and handover limits, GC (split logs, snapshots, unreferenced markers, long-dead leases). Multi-engine tests: convergence, kill, freeze-then-wake (fenced), bucket cut-off (self-fence before takeover), drain, crashed claim, restart, GC; MinIO: self-test and two-node takeover. Gossip is not needed for correctness and is not wired in yet (an accelerator for suspicion, M8); the turmoil harness moves to M9.
- [x] M5 Queue partitions (`queues/{hh}/{queue}`, jump hash for keyed creates, local-first otherwise, exact dedupe across a resize) and request routing: every REST/gRPC call goes through `valka-server::api::Api` (local, or forwarded to the shard owner over `InternalService.Execute` with NOT_OWNER hints and owner-record fallback); lists/counts/workers/stats/shard map are scatter-gathered with an `X-Valka-Partial` header. Internal listener on `cluster.internal_addr` with the cluster token (constant-time) and optional mTLS. Gossip ring, partitions and `ForwardTask` removed. Tests: 7 multi-node integration tests over real localhost gRPC (REST through any of 3 nodes, cross-node idempotency, resize dedupe, bad token, takeover, mTLS, gRPC API).
- [ ] M6 Cross-node matching (offers, claims, grants); result, heartbeat, cancel and signal routing.
- [ ] M7 Worker reattach in the proto and all four SDKs.
- [ ] M8 Drain, rebalance limits, admin API, version gates, metrics, runbooks.
- [ ] M9 Full simulation suite, chaos rig, 24 h soak; release gates green.
- [ ] M10 Helm, systemd, docker-compose cluster, cluster UI phase 2 half, docs.

Independent of phase 2: payload blob externalisation (`blobs/…`) above a size threshold;
`ack=fast` per-task mode.
