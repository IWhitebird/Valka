# Valka WAL Architecture — Design Spec

Status: **implementing on branch `wal`**. This document is the source of truth for the
storage rewrite. Sections marked *(phase 2)* are designed in but exercised only once the
cluster work lands.

## 0. Goal

Replace PostgreSQL with a write-ahead log on an object store (S3 or compatible). Nodes are
diskless and disposable; the bucket is the only truth. Deployment = N × `valka-server` +
one bucket. Dev = one process + a directory.

Public contract (gRPC `ApiService`/`WorkerService`, REST `/api/v1/*`, SDKs, Web UI) is
unchanged.

## 1. The three invariants

1. **Acked = durable.** Nothing is acknowledged to a producer or worker before the WAL
   segment carrying its record has been written to the bucket *and* shard ownership has
   been re-verified. The in-RAM WAL buffer only ever holds unacknowledged work, which
   clients retry.
2. **One writer per shard**, enforced by a CAS'd assignment object with epochs, never by
   gossip. Gossip is a liveness hint only.
3. **Tasks execute on workers, not nodes.** Node death interrupts coordination for
   seconds, never execution. Workers reconnect and reconcile via handshake.

## 2. Sharding

- `NUM_SHARDS = 4096`, fixed. `shard = xxhash64(queue_name) % 4096` for tasks without a
  routing key; with a routing key `xxhash64(queue_name, key) % 4096`.
- The shard id is embedded in the task id (UUIDv7, low 12 bits of `rand_b`) so any node
  can route a `GetTask`/`Cancel`/`Signal` without a lookup.
- Each shard is an independent state machine. No cross-shard transactions.
- The matching service's partition tree (`MatchingConfig.num_partitions`, default 4) is a
  separate, purely in-RAM concept for worker routing and is unchanged.

## 3. Bucket layout

```
assignment                          {shard -> node_id, epoch}  — CAS'd (phase 2)
nodes/{node_id}/lease               liveness lease, CAS'd (phase 2)
wal/{node_id}/{epoch:08}-{seq:016}.seg   group-committed WAL segments
snapshots/{shard:04}/{epoch:08}-{seq:016}.snap   per-shard state snapshot
logs/{run_id}/{seq:08}.log          task log chunks, append-only per run
blobs/{hh}/{task_id}/{input|output} large payloads (threshold-based, later)
```

All keys use the node's `(epoch, seq)` pair as the log sequence number (LSN). Segments
from a node are strictly ordered by LSN. A shard's snapshot at LSN `L` covers every
record for that shard in segments with LSN ≤ `L`.

## 4. WAL segment format

```
header (fixed 32 bytes):
  magic      u32   0x564B4C57  ("VKLW")
  version    u16   1
  flags      u16   bit0 = zstd body
  epoch      u32
  seq        u64
  node_id    [u8;8] (xxhash of node id, sanity only)
  reserved   u32
body (zstd-compressed when flag set):
  repeat:
    len      u32
    crc32    u32   of `payload`
    payload  [u8; len]  = JSON `WalRecord`
```

Records are JSON for debuggability and free CDC (anyone can tail the bucket). zstd
removes the size penalty. Every record carries `shard`, `record_id` (UUIDv7), and the
`ts_ms` it was produced at. Replay never consults the clock; timers that fire produce
their own records (`LeaseExpired`, `Promoted`) so recovery is deterministic.

### Record set

| Record | Fields | Transition |
|---|---|---|
| `TaskCreated` | full task definition | → PENDING (or RETRY-like *scheduled* if `scheduled_at` in future) |
| `TaskDispatched` | task_id, run_id, attempt, worker_id, node_id, lease_until | PENDING → RUNNING, creates run |
| `RunCompleted` | task_id, run_id, output | RUNNING → COMPLETED |
| `RunFailed` | task_id, run_id, error, retryable, next_attempt_at | RUNNING → RETRY / FAILED / DEAD_LETTER |
| `LeaseExpired` | task_id, run_id, next_attempt_at | RUNNING → RETRY / DEAD_LETTER |
| `LeaseExtended` | task_id, run_id, lease_until | heartbeat (coalesced, see §6) |
| `TaskCheckpointed` | task_id, run_id, step, output | records a completed step (see §16) |
| `TaskPromoted` | task_id | RETRY/scheduled → PENDING |
| `TaskCancelled` | task_id, reason | any non-terminal → CANCELLED |
| `TaskDeleted` | task_id | removes task and its runs/DLQ entry |
| `SignalCreated` | signal_id, task_id, name, payload | signal PENDING |
| `SignalDelivered` | signal_id | PENDING → DELIVERED |
| `SignalAcked` | signal_id | DELIVERED → ACKNOWLEDGED |
| `SignalsReset` | task_id | DELIVERED → PENDING (worker disconnect) |
| `QueueCleared` | — | wipes all tasks in the shard (REST `DELETE /tasks`) |

Dedupe: `(task_id, attempt)` for dispatch/complete/fail, `record_id` for everything
else. A record whose precondition no longer holds on replay (e.g. `RunCompleted` for a
task already COMPLETED) is a no-op, never an error.

## 5. Write path (group commit)

```
handler                shard lock                WAL writer task           bucket
   │ validate against RAM state                       │
   │ build record(s)                                   │
   │ apply to RAM state                                │
   │ writer.append(recs) ──── mpsc ────────────────►  buffer, park ack
   │ release lock                                      │ ... 50–100 ms or 4–8 MB ...
   │ await durable ◄──────────────────────────────── PUT seg ────────────► 200
   │                                                   │ verify ownership (ETag GET, phase 2)
   │ ack client                                        │ resolve parked acks (in seq order)
```

- Apply-then-await keeps per-shard ordering trivial: RAM state is always ahead of or equal
  to the durable prefix, and a crash loses only unacked work.
- Reads (`GetTask`, `ListTasks`) serve from RAM. A read can observe a not-yet-durable
  write; that is acceptable because the writer has not been acked and will retry.
- A task is **offered to matching only after** its `TaskCreated` is durable, so a worker
  never runs a task that could vanish.
- Segments are pipelined: while segment `k` is in flight, `k+1` fills. Acks resolve in
  segment order.
- On PUT failure the writer retries with backoff; on persistent failure parked acks fail
  with `UNAVAILABLE` and the RAM state for those records is *not* rolled back (the node
  must restart to reconverge — simple and safe; see Open Questions).

## 6. Heartbeats and leases

Leases are RAM-authoritative. Heartbeats extend the RAM lease immediately and are written
to the WAL **coalesced**: at most one `LeaseExtended` per run per flush interval. On
replay the lease is `max(recorded lease_until, snapshot time + grace)`, and the new owner
grants every replayed RUNNING task a fresh grace lease anyway (workers must re-handshake).

## 7. Timers

One timer wheel per engine (BinaryHeap keyed by fire time) holding
`{Promote(task), LeaseExpiry(run)}`. Ticks every 100 ms (tokio time, pausable in tests).
Firing produces a record and applies it like any other command. Timer state is derived
from task state on replay, never persisted separately.

## 8. Snapshots and truncation

- Every `snapshot_interval` (default 60 s) or `snapshot_after_records` (default 50k),
  each dirty shard is serialized (`ShardSnapshot` = tasks, runs, signals, dead letters,
  idempotency map) as zstd JSON to `snapshots/{shard}/{lsn}.snap`.
- A shard's snapshot LSN is the writer's LSN at the moment the shard lock was held, so it
  is exact.
- Segments with LSN < min(snapshot LSN over owned shards) are deleted after a grace
  period. Old snapshots beyond the latest 2 are deleted.

## 9. Recovery (single node, phase 1)

1. Claim all 4096 shards in `assignment` (CAS create-or-update, epoch += 1).
2. For each shard: latest snapshot → load; else empty.
3. List `wal/{node_id}/` segments with LSN > min snapshot LSN; read in order; for each
   record apply if `record.shard`'s snapshot LSN < segment LSN.
4. Rebuild timers from state. Mark all RUNNING tasks with a grace lease.
5. Start serving. Workers reconnect; their `WorkerHello` may carry `running_task_ids`
   *(SDK change, additive)*; runs not reclaimed within the grace window expire normally.

## 10. Task logs

Workers stream `LogBatch`; the ingester buffers per `run_id` and writes
`logs/{run_id}/{seq:08}.log` (zstd JSON lines) every 500 ms / 100 entries. Reads list
the prefix and concatenate. Logs are not part of the WAL.

## 11. Visibility (`ListTasks`, dead letters)

Served from RAM by scanning shards with filters and offset paging. Bounded by RAM by
design: a node's working set is its owned shards' live tasks. Completed tasks are
retained in RAM for `completed_retention` (default 24 h) then dropped from the shard
state (their history remains in the WAL/snapshots in the bucket).

## 12. Fencing *(phase 2)*

Superseded by [`PHASE2.md`](PHASE2.md): per-shard owner records and generations, fencing by
each node's own lease, sealing at the first gap, and log splitting. This section describes
the phase-1 single-object design that PHASE2.md replaces.

- `assignment` is a single JSON object `{version, shards: [{node, epoch}; 4096]}` written
  with `If-Match` (object_store `PutMode::Update`).
- Flush protocol: PUT segment → GET `assignment` with `If-None-Match: <cached etag>` →
  304 ⇒ still owner ⇒ release acks; 200 with changed ownership ⇒ drop acks for lost
  shards (clients retry to new owner) and stop writing those shards.
- Takeover: gossip marks node dead → peer CASes assignment (epoch+1 for those shards) →
  loads snapshots → lists dead node's segments → replays with epoch filter → serves.
- Segment PUTs are create-if-absent (`If-None-Match: *`). On takeover the new owner
  **seals** the old epoch by writing an empty segment at `(old_epoch, last_seen_seq + 1)`
  before it starts its own epoch. A zombie's late PUT for that position then fails, and
  any position beyond it is never read because replay stops at the first gap. Without
  sealing, a late zombie segment would sort before the new epoch on the next recovery
  and be replayed even though nothing in it was ever acknowledged.

## 13. Test strategy

- `valka-wal` and `valka-engine` unit tests run on `object_store::memory::InMemory` with a
  **fault-injecting store wrapper** (`FaultyStore`): per-op latency, error rate, "PUT
  succeeds but the response is lost", crash-before-ack.
- Deterministic time: `tokio::time::pause()` in engine tests; `Clock` trait for
  wall-clock.
- **Crash/replay property tests**: run a random command sequence against an engine, kill
  it at a random point, rebuild from the bucket, assert the recovered state equals the
  state implied by the acked prefix. Run under `proptest` with many seeds.
- REST/gRPC tests use `build_api_router` with an in-memory store — no external services.
- MinIO (docker compose) tests, feature `minio`, exercise real conditional PUT semantics.
- Phase 2 adds `turmoil` for partitions / split-brain.

## 14. Trade-offs accepted (from the design session)

| Give up | Get | Knob |
|---|---|---|
| 1–5 ms enqueue ack → 50–200 ms | diskless nodes, 11-nines durability | flush interval; S3 Express; `ack=fast` |
| SQL over live state | free CDC, unlimited history | visibility sink later |
| instant failover → ~10–20 s per affected shard | any node killable | more shards, lazy load |
| disk-backed working set | predictable RAM hot path | backpressure |
| cross-shard atomicity | linear scale | shard by key |
| global idempotency index | lock-free local dedupe | TTL window |
| DB-inherited correctness | a fencing protocol we own | simulation tests |

## 15. Open questions

- Roll back RAM state on persistent PUT failure vs. restart the node? Phase 1: restart.
- Payload blob threshold (probably 64 KB) — deferred.
- `ack=fast` per-task mode — deferred.

## 16. Step checkpoints

A running run may record that it completed a named step: `WorkerService.Checkpoint`
(`task_id`, `task_run_id`, `step`, JSON `output`) → `TaskCheckpointed`, acknowledged only
once durable, like `RunCompleted`.

- **Fencing.** The engine accepts a checkpoint only from the task's current RUNNING run;
  `apply` re-checks the same precondition, so a stale run (lease expired, cancelled,
  finished) can never add or overwrite one, at runtime or on replay.
- **State.** `TaskState.checkpoints` holds steps in first-completion order; checkpointing a
  step again updates it in place. Checkpoints are part of the task, so snapshots, eviction
  and `TaskDeleted` cover them with no extra machinery.
- **Resume.** Every dispatch carries the task's checkpoints in `TaskAssignment.checkpoints`.
  The SDKs' `step(name, fn)` returns a recorded result without running `fn`, otherwise runs
  it and checkpoints the result before returning.
- **Semantics.** A checkpointed step never re-runs. The step in progress when an attempt
  dies runs again (at-least-once), so steps with external side effects must be idempotent.
- **Limits.** Step names 1–256 bytes, outputs ≤ 256 KiB of JSON, ≤ 256 distinct steps per
  task (`valka_engine::MAX_*`); violations are `INVALID_ARGUMENT`.
- **Read path.** `GET /api/v1/tasks/{id}/checkpoints`; the task page shows a Steps table.
