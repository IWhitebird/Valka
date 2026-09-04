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

## Phase 2 — cluster

- [ ] Assignment CAS + epochs, flush-time ownership verification.
- [ ] Node lease + gossip-triggered takeover; snapshot + tail replay of dead node.
- [ ] Worker reconnect handshake (`running_task_ids` in `WorkerHello`); reconciliation window before re-dispatch.
- [ ] Forwarding by shard owner (replace hash ring partitions with assignment table).
- [ ] turmoil simulation tests: partitions, split-brain, zombie writer.
- [ ] Restore multi-node deploy assets (`deploy/docker/docker-compose.cluster.yml`, Helm `replicaCount > 1`).
- [ ] Payload blob externalisation (`blobs/…`) above a size threshold; `ack=fast` per-task mode.
