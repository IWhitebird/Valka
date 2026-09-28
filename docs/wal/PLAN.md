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
- [ ] M2 Formats: `cluster.json`, 16-bit shard ids, `gen` in records, sealable segments, `owners/`, `nodes/`, new snapshot keys.
- [ ] M3 One node on the full protocol: incarnations, lease fencing, per-shard claim/release/split.
- [ ] M4 Membership, guardian tombstones, rendezvous placement, takeover, GC; turmoil harness starts.
- [ ] M5 Queue partitions and request routing; internal RPCs with token + mTLS.
- [ ] M6 Cross-node matching (offers, claims, grants); result, heartbeat, cancel and signal routing.
- [ ] M7 Worker reattach in the proto and all four SDKs.
- [ ] M8 Drain, rebalance limits, admin API, version gates, metrics, runbooks.
- [ ] M9 Full simulation suite, chaos rig, 24 h soak; release gates green.
- [ ] M10 Helm, systemd, docker-compose cluster, cluster UI phase 2 half, docs.

Independent of phase 2: payload blob externalisation (`blobs/…`) above a size threshold;
`ack=fast` per-task mode.
