# Valka WAL Architecture — Design Spec

Status: **implemented on branch `wal`**. This document specifies one node: the WAL, the
engine, snapshots and the worker protocol. How many nodes share a bucket (leases, owner
records, claims, splits, routing, cross-node workers) is specified in
[`PHASE2.md`](PHASE2.md), also implemented. A readable overview of both:
[`../ARCHITECTURE.md`](../ARCHITECTURE.md).

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
2. **One writer per shard**, enforced by per-shard owner records moved only by CAS and by
   each node's own lease, re-checked after every segment PUT (PHASE2 §8, §12.3). Nothing
   else (no gossip, no peer messages) decides ownership.
3. **Tasks execute on workers, not nodes.** Node death interrupts coordination for
   seconds, never execution. Workers reconnect and reconcile via handshake.

## 2. Sharding

- The shard count is chosen when a bucket is first used (`cluster.num_shards`, default
  4096, a power of two in 16..=65,536) and stored in `cluster.json`; it never changes.
- A queue has partitions (`queues/{hh}/{queue}`, default 16). Partition `p` of queue `Q`
  lives on shard `xxhash64(Q, p) mod num_shards`. A create with an idempotency key picks
  `jump_hash(key, partitions)`; one without picks a partition whose shard this node owns,
  else a random one (PHASE2 §9).
- The shard id is embedded in the task id (UUIDv7, low 16 bits) so any node can route a
  `GetTask`/`Cancel`/`Signal` without a lookup. Run and signal ids are minted in their
  task's shard.
- Each shard is an independent state machine. No cross-shard transactions.
- The matching service's partition tree (`MatchingConfig.num_partitions`, default 4) is a
  separate, purely in-RAM concept for worker routing on one node.

## 3. Bucket layout

```
cluster.json                                     cluster id, num_shards, format (created once)
nodes/{node}/lease                               incarnation, state, counter, trunc, addrs; CAS'd
owners/{hh}/{shard}                              {node, inc, gen, owned|released}; CAS'd on moves
queues/{hh}/{queue}                              partition count; CAS'd by admin changes
wal/{node}/{inc:010}-{seq:016}.seg               group-committed WAL segments, create-only
snapshots/{hh}/{shard}/{gen}-{seq}-{ptr}.snap    per-shard state; ptr = node.inc.seq | final | split
splits/{node}/{inc}                              "this dead incarnation's log is split"
logs/{hh}/{run_id}/{uuidv7}.log                  task log chunks
placement.json                                   operator shard pins (admin API)
admin/{ts}-{id}.json                             admin audit records
blobs/{hh}/{task_id}/{input|output}              large payloads (threshold-based, later)
```

`{hh}` is two hex digits of a hash of the rest of the key, spreading hot key families over
S3 partitions. `object_store` lists by path segment, so every LIST prefix ends at a `/`.

A node's log position (LSN) is `(inc, seq)`: its incarnation and a sequence number. Segments
of an incarnation are strictly ordered by `seq`. A snapshot key names the shard's generation,
the last `shard_seq` it covers, and where replay continues (a position in its owner's log,
or `final` / `split` when nothing follows).

## 4. WAL segment format

```
header (little-endian):
  magic        u32   0x564B4C57  ("VKLW")
  version      u16   2
  flags        u16   bit0 = zstd body, bit1 = SEALED (no records)
  inc          u32
  seq          u64
  record_count u32
  header_len   u32
  node_len     u16, node id bytes
  shard_count  u32, shard u16 × shard_count     (sorted, distinct)
  header_crc   u32   (crc32 of every header byte before it)
body (zstd-compressed when flag set):
  repeat:
    len      u32
    crc32    u32   of `payload`
    payload  [u8; len]  = JSON `Envelope`
```

Records are JSON for debuggability and free CDC (anyone can tail the bucket). zstd
removes the size penalty. Every envelope carries `shard`, `shard_seq` (per shard, no gaps),
`gen` (the shard's ownership generation), `record_id` (UUIDv7) and the `ts` it was produced
at. Replay never consults the clock; timers that fire produce their own records
(`LeaseExpired`, `TaskPromoted`) so recovery is deterministic. The shard list in the
header lets a splitter skip segments without the shards it rebuilds.

### Record set

| Record | Fields | Transition |
|---|---|---|
| `TaskCreated` | full task definition | → PENDING (or RETRY-like *scheduled* if `scheduled_at` in future) |
| `TaskDispatched` | task_id, run_id, attempt, worker_id, node_id, lease_until | PENDING → RUNNING, creates run |
| `RunCompleted` | task_id, run_id, output | RUNNING → COMPLETED |
| `RunFailed` | task_id, run_id, error, retryable, next_attempt_at | RUNNING → RETRY / FAILED / DEAD_LETTER |
| `LeaseExpired` | task_id, run_id, next_attempt_at | RUNNING → RETRY / DEAD_LETTER |
| `LeaseExtended` | task_id, run_id, lease_until | heartbeat (coalesced, see §6) |
| `RunReattached` | task_id, run_id, worker_id, node_id, lease_until | a reconnected worker's run now lives on another node (PHASE2 §11.4) |
| `RunReturned` | task_id, run_id | RUNNING → PENDING without spending an attempt (the run never reached its worker) |
| `TaskCheckpointed` | task_id, run_id, step, output | records a completed step (see §16) |
| `TaskPromoted` | task_id | RETRY/scheduled → PENDING |
| `TaskCancelled` | task_id, reason | any non-terminal → CANCELLED |
| `TaskDeleted` | task_id | removes task and its runs/DLQ entry |
| `SignalCreated` | signal_id, task_id, name, payload | signal PENDING |
| `SignalDelivered` | signal_id | PENDING → DELIVERED |
| `SignalAcked` | signal_id | DELIVERED → ACKNOWLEDGED |
| `SignalsReset` | task_id | DELIVERED → PENDING (worker disconnect) |
| `ShardCleared` | — | wipes all tasks in the shard (REST `DELETE /tasks`) |

Replay applies a record only if its `gen` equals the base snapshot's and its `shard_seq`
is above the snapshot's, so it never relies on idempotency. A record whose precondition
no longer holds (e.g. `RunCompleted` for a task already COMPLETED) is a no-op, never an
error.

## 5. Write path (group commit)

```
handler                shard lock                WAL writer task           bucket
   │ validate against RAM state                       │
   │ build record(s)                                   │
   │ apply to RAM state                                │
   │ writer.append(recs) ──── mpsc ────────────────►  buffer, park ack
   │ release lock                                      │ ... 50–100 ms or 4–8 MB ...
   │ await durable ◄──────────────────────────────── PUT seg ────────────► 200
   │                                                   │ own-lease check (conditional GET)
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
  with `UNAVAILABLE` and the RAM state for those records is *not* rolled back. The writer
  is poisoned and the server exits non-zero, so its supervisor restarts it and it rebuilds
  from the bucket (see Open Questions).
- A retried segment PUT that finds the position taken succeeds only if the stored bytes are
  its own (its earlier attempt landed with the response lost); anything else is fatal.

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
  idempotency map) as zstd JSON to `snapshots/{hh}/{shard}/{gen}-{shard_seq}-{ptr}.snap`.
- **Log budget:** once `log_budget_bytes` (default 64 MiB) of segments have been
  committed since the last full round, every dirty shard is snapshotted (checked every
  second). This bounds restart replay whatever the write pattern.
- **Snapshots only cover durable records.** Shards are captured in batches of 64, then the
  writer is synced unconditionally. If that sync fails, the batch is abandoned and nothing
  is written, so a snapshot can never resurrect a write whose caller was told it failed.
- A shard's snapshot names the last `shard_seq` applied and the writer's next position at
  the moment the shard lock was held, so it is exact.
- **Truncation:** the node first CASes its lease's `trunc` watermark up to the lowest log
  pointer over its serving shards' latest snapshots (and at most its acked position + 1),
  then deletes its own segments below it, ascending. A split reads a log only from that
  watermark up, so a late delete can never remove something a split needs (PHASE2 §8.7).
- Old snapshots beyond the latest 2 per shard are deleted; the newest never is.

## 9. Recovery

Restart and takeover are one code path (PHASE2 §7.2, §8.3, §8.4):

1. CAS the node's lease to a new incarnation (`inc + 1`); this fences the previous process.
2. Split each earlier incarnation of this node that is not split yet: read its log from its
   watermark to the first gap, seal the gap, rebuild every shard it owned from that shard's
   newest snapshot plus its records, write split snapshots and the split marker.
3. Claim shards (placement decides which): CAS the owner record to `gen + 1`, load the
   newest snapshot, write an acquisition snapshot pointing into the new log, serve.
4. Rebuild timers and the pending index from state. RUNNING tasks get a grace lease;
   recovered PENDING tasks wait `reconcile_grace` so reconnecting workers can reattach
   (`WorkerHello.running`) before anything is re-dispatched.

## 10. Task logs

Workers stream `LogBatch`; the ingester buffers per `run_id` and writes
`logs/{hh}/{run_id}/{uuidv7}.log` (zstd JSON lines) every 500 ms / 100 entries. Reads list
the prefix and concatenate. Logs are not part of the WAL.

## 11. Visibility (`ListTasks`, dead letters)

Served from RAM by scanning shards with filters and offset paging. Bounded by RAM by
design: a node's working set is its owned shards' live tasks. Completed tasks are
retained in RAM for `completed_retention` (default 24 h) then dropped from the shard
state (their history remains in the WAL/snapshots in the bucket).

## 12. Fencing

Specified in [`PHASE2.md`](PHASE2.md) §7 and §12: every node has a lease it renews by CAS
every 2 s and stops serving 7 s after its last successful renewal; each shard has an owner
record naming one incarnation and a generation; after every segment PUT the writer re-reads
its own lease (conditional GET) before releasing acks, and a lease taken by a guardian's
tombstone or a newer incarnation fails the acks and poisons the node. A dead incarnation's
log is sealed at its first gap, so its late PUTs can't extend it. Segment PUTs are
create-only; a retried PUT that finds its position taken succeeds only if the stored bytes
are its own.

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
- Many nodes: multi-engine tests on one in-memory bucket (`cluster_tests.rs`,
  `safety_tests.rs`), a randomized simulation with a history checker (`sim_tests.rs`), a
  TLA+ model (`spec/tla/`), 3-node integration tests over real gRPC, and a docker compose
  chaos rig (`scripts/chaos.sh`). See PHASE2 §15.

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

- Roll back RAM state on persistent PUT failure vs. restart the node? Phase 1: the node
  exits and its supervisor restarts it.
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

## 17. Worker protocol: results, drain and server restarts

- **Results are acknowledged end to end.** Every `TaskResult` is answered with a
  `ResultAck`: `APPLIED` (durable, now or from an earlier delivery), `STALE` (the run
  already ended another way: lease expired, cancelled, deleted) or `RETRY` (not recorded;
  send again). `Engine::report_result` is idempotent, and answers `APPLIED` or `STALE` for
  an already-ended run only after a WAL sync, so it never confirms state that exists only
  in RAM.
- **SDKs keep a result until APPLIED or STALE.** They resend it on `RETRY`, on reconnect
  (Go also on every heartbeat tick), and keep heartbeating the task meanwhile so its lease
  does not expire. A handler that finishes while the server is down therefore still gets
  its result recorded once the server is back, without re-running the task.
- **Everything a running task sends follows the current connection:** results, logs,
  signal acks and checkpoints. A task that outlives a reconnect keeps reporting.
- **Slots never block the receive loop.** A handler takes its concurrency slot inside its
  own task, so acks, signals and cancellations keep flowing at full capacity.
- **Worker drain.** `GracefulShutdown` means "stop sending me tasks": the server removes the
  worker from matching but keeps the stream, so results of in-flight tasks and their acks
  still flow. The worker closes the stream once its handlers are done and their results
  acknowledged (or after 5 s).
- **Server shutdown.** On SIGTERM every worker stream gets `ServerShutdown` and is closed,
  so the server stops promptly instead of waiting for streams. Workers treat it as a
  disconnect and reconnect with backoff; they do not exit.

