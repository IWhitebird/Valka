# WAL rewrite — implementation plan

## Phase 1 — single node (this branch, now)

- [ ] `valka-core`: drop sqlx; `StorageConfig` (backend, bucket/path, flush knobs); shard helpers; `ServerError::Storage`.
- [ ] `valka-wal` (new): `Store` wrapper over `object_store` + `FaultyStore`; `WalRecord`; segment codec; `WalWriter` (group commit, parked acks, pipelining); `WalReader`; snapshot codec; `Ownership` (assignment CAS, single-node path).
- [ ] `valka-engine` (new): `ShardState` + apply; `Engine` command API (create/get/list/cancel/delete/clear/signal/dispatch/complete/fail/heartbeat); timer wheel; recovery; snapshotter; log store.
- [ ] `valka-dispatcher`: replace PG calls with `Engine`.
- [ ] `valka-matching`: `TaskReader` → in-RAM feeder from `Engine` pending heaps.
- [ ] `valka-scheduler`: delete (reaper/retry/DLQ/promoter live in engine timers).
- [ ] `valka-server`: rewire gRPC/REST/internal; log ingester → log store; remove migrations.
- [ ] `valka-db`: delete.
- [ ] Tests: rewrite unit tests; replace PG integration suites with in-memory engine + REST suites; crash/replay proptests; MinIO feature.
- [ ] docker-compose: MinIO instead of Postgres; CI update; docs.

## Phase 2 — cluster

- [ ] Assignment CAS + epochs, flush-time ownership verification.
- [ ] Node lease + gossip-triggered takeover; snapshot + tail replay of dead node.
- [ ] Worker reconnect handshake (`running_task_ids` in `WorkerHello`); reconciliation window before re-dispatch.
- [ ] Forwarding by shard owner (replace hash ring partitions with assignment table).
- [ ] turmoil simulation tests: partitions, split-brain, zombie writer.
