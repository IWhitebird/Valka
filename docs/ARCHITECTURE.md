# How Valka works

This is the guide to read first. It explains Valka from the outside in: what a node is,
how a task moves through the system, how several nodes share one bucket without ever
losing acknowledged work, and what happens when things fail. The precise specs are
[`wal/DESIGN.md`](wal/DESIGN.md) (single node: WAL, engine, worker protocol) and
[`wal/PHASE2.md`](wal/PHASE2.md) (many nodes: the ownership protocol and why each step is
safe). Operating a cluster: [`wal/RUNBOOKS.md`](wal/RUNBOOKS.md).

Contents

1. [The one rule](#1-the-one-rule)
2. [The big picture](#2-the-big-picture)
3. [Shards, partitions and ids](#3-shards-partitions-and-ids)
4. [Inside one node](#4-inside-one-node)
5. [The write path: acked means durable](#5-the-write-path-acked-means-durable)
6. [Life of a task](#6-life-of-a-task)
7. [Snapshots, truncation and restart](#7-snapshots-truncation-and-restart)
8. [Many nodes: who owns which shard](#8-many-nodes-who-owns-which-shard)
9. [Requests and workers across nodes](#9-requests-and-workers-across-nodes)
10. [Failure walkthroughs](#10-failure-walkthroughs)
11. [What is guaranteed, and what is not](#11-what-is-guaranteed-and-what-is-not)
12. [Timings and knobs](#12-timings-and-knobs)
13. [How it is verified](#13-how-it-is-verified)
14. [Compared with Cloudflare Durable Objects](#14-compared-with-cloudflare-durable-objects)
15. [Where the code is](#15-where-the-code-is)
16. [Glossary](#16-glossary)

---

## 1. The one rule

**The bucket is the only truth.** Valka keeps nothing important on a node's disk and uses
no database, no Raft, no ZooKeeper, no message broker. Everything that matters is an
object in one S3-compatible bucket (or a local directory, for a single dev node):

- every change to a task is a record in a **write-ahead log** (WAL) stored as objects;
- the live state is held **in RAM** by whichever node owns it, rebuilt from the bucket
  whenever ownership moves;
- who owns what, and who is alive, is decided by **conditional writes** (compare-and-swap
  on an object's ETag) in the same bucket.

A node is therefore disposable: kill it, and another node rebuilds its state from the
bucket. This costs some latency (an acknowledgement waits for an S3 PUT, typically
50–200 ms with the default 50 ms group-commit window) and buys durability, simple
operations and a cluster that needs nothing but a bucket.

## 2. The big picture

```
   producers (REST / gRPC / CLI / SDK clients)          workers (Rust, Go, Python, TS SDKs)
          │  create, get, cancel, list, signal                │  one bidirectional stream each
          ▼                                                   ▼
 ┌──────────────── node A ────────────────┐   ┌──────────────── node B ────────────────┐
 │ API layer: run here or forward ────────┼──►│ API layer                              │
 │ Dispatcher: worker streams             │◄──┼─ internal gRPC (token, optional mTLS): │
 │ Matching: workers waiting per queue    │   │  Execute, Scatter, Events              │
 │ Engine: the shards A owns, in RAM      │   │ Engine: the shards B owns, in RAM      │
 │   timers, pending index, WAL writer    │   │   timers, pending index, WAL writer    │
 └──────────────────┬─────────────────────┘   └──────────────────┬─────────────────────┘
                    │ PUT segments, CAS lease/owners, snapshots   │
                    ▼                                             ▼
 ┌───────────────────────────────── the bucket ─────────────────────────────────────────┐
 │ cluster.json                     cluster id, shard count (written once)              │
 │ nodes/{node}/lease               who is alive: incarnation, state, renew counter     │
 │ owners/{hh}/{shard}              which node incarnation owns a shard, and generation │
 │ wal/{node}/{inc}-{seq}.seg       each node's own log, append-only                    │
 │ snapshots/{hh}/{shard}/…         per-shard state checkpoints                         │
 │ splits/{node}/{inc}              "this dead log has been redistributed"              │
 │ queues/{hh}/{queue}              a queue's partition count                           │
 │ logs/{hh}/{run}/…                task log lines (not part of the WAL)                │
 │ placement.json, admin/…          operator pins, admin audit records                  │
 └──────────────────────────────────────────────────────────────────────────────────────┘
```

Things to notice:

- **Any node accepts any request.** A request for a task another node owns is forwarded
  to that node over the internal gRPC listener (default `:50052`).
- **Workers can attach to any node** and run tasks owned by any node.
- **Nodes never ask each other who is alive or who owns what.** They read it from the
  bucket. Node-to-node calls only move requests and tasks around; if those calls fail,
  requests are retried, but nothing about ownership changes.
- **One node is a cluster of one.** A single `valka-server` runs exactly the same
  protocol, alone. There is no separate single-node code path.

## 3. Shards, partitions and ids

**Shards.** All state is split into a fixed number of shards (default 4096, chosen when the
bucket is first used and stored in `cluster.json`). A shard is the unit of ownership: at
any moment exactly one node serves it, holds its state in RAM and writes its records. A
shard is a small state machine (`ShardState`): its tasks, runs, signals, dead letters and
idempotency keys. Nothing ever spans two shards, so no operation needs coordination
between shards.

**Queue partitions.** A queue is split into partitions (default 16, stored in
`queues/{hh}/{queue}`, changed by an admin call). Partition `p` of queue `Q` lives on shard
`hash(Q, p) mod num_shards`, so a queue's tasks are spread over up to 16 shards, which may
be owned by up to 16 different nodes.

When a task is created the receiving node picks a partition:

- with an idempotency key: `jump_hash(key, partitions)`, so the same key always lands on
  the same shard and deduplication is exact (also across a partition-count change: the old
  partition is checked as well);
- without a key: preferably a partition whose shard this node owns (no network hop),
  otherwise a random one.

**Ids carry their shard.** Task ids are UUIDv7 (time-ordered), with the shard number
embedded in the low 16 bits. Any node can route `GET /tasks/{id}`, a cancel or a signal
straight to the right shard without looking anything up. Run ids are minted in the same
shard as their task.

```
create "send-email" in queue "emails"
  → queue config: 16 partitions → partition 11 (no key: one this node owns)
  → shard = hash("emails", 11) mod 4096 = 2731
  → task id = UUIDv7 with 2731 in its low bits: 019a…-…-…0aab
```

## 4. Inside one node

A `valka-server` process runs these pieces (all in one tokio runtime):

| Piece | Crate | What it does |
|---|---|---|
| REST API (`:8989`) and gRPC API (`:50051`) | `valka-server` | public endpoints; the dashboard is served from the same port |
| API layer | `valka-server::api` | decides where a request runs: locally, forwarded to the owner, or scattered to every node and merged |
| Internal listener (`:50052`) | `valka-server::internal_grpc` | answers other nodes' `Execute` / `Scatter` / `Events` calls |
| Engine | `valka-engine` | the shards this node owns, the command API, timers, the pending index, snapshots, recovery and the control loop |
| WAL writer | `valka-wal` | batches records into segments and PUTs them (group commit) |
| Lease keeper | `valka-wal::lease` | renews this node's lease every 2 s |
| Dispatcher | `valka-dispatcher` | one bidirectional gRPC stream per worker: assignments, results, heartbeats, signals, cancels |
| Matching | `valka-matching` | which local workers are waiting for which queue; hands a task to one of them |
| Remote feeder | `valka-server::feeder` | when local workers wait for a queue whose partitions other nodes own, long-polls those owners for work |
| Event relay | `valka-server::relay` | streams other nodes' task events to this node's subscribers (SSE, gRPC) |
| Log ingester | `valka-engine::ingest` | batches workers' log lines into `logs/…` objects |

The engine's background loops: the **ticker** (every 100 ms: lease expiry, retry
promotion, retention, coalesced heartbeat records), the **feeder** (offers runnable tasks
to waiting workers), the **snapshotter**, and the **control loop** (membership, placement,
guardian duties, garbage collection).

## 5. The write path: acked means durable

Every change, whether a create, a dispatch, a result, a cancel, a signal or a timer firing,
goes through one function, `Engine::mutate`:

```
 caller                 shard lock                       WAL writer                 bucket
   │  lock the shard                                          │
   │  validate against the RAM state                          │
   │  build the record, ShardState::apply it                  │
   │  writer.append(record) ───────────────────────────────►  │ buffer (≤ 50 ms or 4 MB)
   │  unlock                                                  │
   │                                                          │ PUT wal/{me}/{inc}-{seq}.seg ──► 200
   │                                                          │ GET my own lease (conditional) ──► still mine
   │  ◄────────────────────────────────── durable ────────────│ release acks, in order
   │  after_durable: publish events, offer to matching   (in its own task: it runs even if
   │  acknowledge the client                              the caller has gone away)
```

- **Apply, then wait.** The RAM state is updated under the shard lock before the record is
  durable, so a shard's records are produced in order and RAM is always at or ahead of the
  durable log. If the PUT never succeeds, the caller is told it failed and the node exits
  (its supervisor restarts it and the bucket is the truth again), so a write nobody was
  told succeeded can't leak into a later answer.
- **Group commit.** One segment carries every record appended during the window, from all
  shards, so throughput is bounded by PUT bandwidth, not PUT latency. Up to 4 segments are
  in flight at once; acknowledgements are still released strictly in order.
- **The own-lease check.** After each segment PUT, before releasing its acks, the writer
  does a conditional GET of its own lease (`nodes/{me}/lease`). If another node has
  declared this one dead, or a newer process took the same node id, the check fails, the
  acks fail, and the node exits. That one request per flush is the whole write fence: it
  needs no clock, and its cost does not depend on the cluster size (§8.4 explains why it
  is enough).
- **Only durable work is visible to workers.** A task is offered to matching only after
  its `TaskCreated` record is durable, so a worker never runs a task that could vanish.
- **`apply` is total.** A record whose precondition no longer holds (for example a
  `RunCompleted` for a run that was already cancelled) is a no-op, at runtime and during
  replay. Replay never consults the clock: timers that fire produce their own records
  (`LeaseExpired`, `TaskPromoted`), so rebuilding a shard always gives the same state.

Every record carries the shard, a per-shard sequence number (`shard_seq`), the shard's
**generation** (`gen`, bumped whenever the shard changes owner), a record id and a
timestamp. Segments are zstd-compressed, CRC-framed JSON, so anyone can read the log.

## 6. Life of a task

### 6.1 On one node

```
producer          node (owns shard 2731)                          worker
   │ CreateTask        │                                             │
   │──────────────────►│ place: partition 11 → shard 2731, mint id   │
   │                   │ mutate: TaskCreated ── WAL ── durable       │
   │◄── task (PENDING) │                                             │
   │                   │ matching: a worker is waiting on "emails" ─►│ TaskAssignment (run r1)
   │                   │ TaskDispatched (async, at-least-once)       │
   │                   │◄───────────────────── heartbeats ───────────│ (extend the run's lease)
   │                   │◄───────────────────── Checkpoint(step) ─────│ (optional, acked once durable)
   │                   │◄───────────────────── TaskResult(ok) ───────│
   │                   │ mutate: RunCompleted ── WAL ── durable      │
   │                   │──────────────────── ResultAck(APPLIED) ────►│
```

State machine:

```
PENDING → RUNNING → COMPLETED
                  → FAILED        (non-retryable error)
                  → RETRY → PENDING        (after backoff, promoted by the timer)
                  → DEAD_LETTER   (out of attempts)
any non-terminal → CANCELLED
```

- If no worker is waiting, the task stays in the shard's **pending index** (by priority,
  then age). The feeder offers it as soon as a worker for that queue becomes free.
- A run holds a **lease**, extended by worker heartbeats. If the worker vanishes, the timer
  fires `LeaseExpired` and the task is retried (or dead-lettered).
- Results are **acknowledged end to end**: `APPLIED` (durable), `STALE` (the run had
  already ended another way, e.g. cancelled or expired) or `RETRY` (send again). SDKs keep
  a result until it is `APPLIED` or `STALE`, so a result produced while the server was
  restarting is not lost and the task is not re-run.
- **Steps** (`ctx.step("name", fn)`) record a checkpoint after each completed step; a retry
  receives the checkpoints and skips finished steps.
- **Signals** (`POST /tasks/{id}/signal`) are stored in the shard and delivered to the
  worker running the task; the worker acknowledges them.

### 6.2 Across nodes

With three nodes, the producer may talk to node A, the task's shard may be owned by B, and
the only free worker may be attached to C:

```
producer ─► A: CreateTask
            A: place → shard 2731; owner (from the owner record cache) is B
            A ─ Execute(CreateTask, id) ─► B: TaskCreated, durable, reply ─► A ─► producer

worker w on C is idle on "emails"; C's remote feeder long-polls the owners of the
partitions of "emails":
            C ─ Fetch("emails") ─────────► B: picks a runnable task, reserves it for C
                                             (in RAM only; the reservation lapses after 30 s)
            C: offers it to w; w accepts
            C ─ Dispatch(task, run id, worker w, node C) ─► B: TaskDispatched (worker w, node C)
            C ─► w: TaskAssignment

w runs it:  heartbeats  C ─► B   (batched per owner per tick)
            checkpoint  C ─► B   (acked once durable)
            result      C ─► B: RunCompleted, durable ─► ResultAck ─► C ─► w
cancel:     producer ─► any node ─► B: TaskCancelled ─► B tells C (the run's node) ─► w
```

- A task C fetched but could not hand to a worker goes back (`Unoffer`). A run B recorded
  that never reached its worker goes back with `RunReturned`, without spending an attempt.
- The owner records which node each run's worker is on, so cancels and signals go to that
  node. If the worker reconnects elsewhere, it **reattaches** (§9.3) and the record is
  updated.
- Event subscribers on any node see every node's events through the event relay.

## 7. Snapshots, truncation and restart

The WAL grows forever unless it is cut. The **snapshotter** writes each changed shard's
whole state to `snapshots/{hh}/{shard}/{gen}-{shard_seq}-{ptr}.snap` every 60 s, after
50,000 records, or once 64 MiB of log has accumulated (the **log budget**), whichever
comes first. The key says exactly what the snapshot covers: the shard's generation, the
last applied `shard_seq`, and a pointer into the log where replay must continue.

- A snapshot covers only **durable** records: the shard is captured, then the writer is
  synced unconditionally; if that sync fails, nothing is written.
- Replay skips records at or below the snapshot's `shard_seq` and records of other
  generations, so it never depends on records being idempotent.
- **Truncation:** once every shard a node serves has a snapshot pointing past a segment,
  that segment is deleted. Before deleting, the node raises a **watermark** (`trunc`) in
  its lease with a CAS; a log is only ever read from that watermark up. Deletes go in
  ascending order.
- Old snapshots are pruned (the newest two per shard are kept; the newest is never
  deleted).

**Restart of a node** (same `VALKA_NODE_ID`): the new process CASes its lease to a new
**incarnation** (`inc + 1`), which fences the previous process if it is somehow still
running. It then **splits** its previous incarnation's log (§8.5), which makes its shards
claimable at once, without waiting for any lease timeout. A node alone claims them right
back. In a cluster, live peers claim them immediately, and placement moves the restarted
node's share back to it once it has been up for the join delay (15 s) and the shards have
stayed put for the minimum residency (5 min). Restart cost is bounded by the log budget: at
most 64 MiB of log per node to read.

## 8. Many nodes: who owns which shard

This is the core of the design. The question is: how do nodes that share nothing but a
bucket agree that exactly one of them writes each shard, even when nodes pause for minutes,
networks split, and S3 responses get lost?

### 8.1 Leases: who is alive

Each node owns one object, `nodes/{node}/lease`:

```json
{ "node": "valka-2", "inc": 7, "state": "alive", "counter": 18342, "trunc": 120,
  "unsplit": [], "internal_addr": "valka-2:50052", "grpc_addr": "…", "http_addr": "…",
  "version": "0.2.0", "protocol": 2, "started_at": "…" }
```

`trunc` is the current incarnation's watermark (§7); `unsplit` lists earlier incarnations
of this node whose logs still need splitting, each with its own frozen watermark.

- **Start:** CAS the lease to `inc + 1`. Two processes with the same node id can't both
  succeed; the old one is fenced.
- **Renew:** every 2 s, CAS `counter + 1`. A CAS conflict means somebody else wrote the
  lease: if it is still mine (my earlier write landed, only the response was lost) retry,
  otherwise I am fenced and exit.
- **Self-fence:** if a node has not renewed successfully for 7 s (`lease_timeout` 10 s
  minus a 3 s safety margin), it stops serving and answers `UNAVAILABLE` until it renews
  again. It goes quiet before anybody may take over its shards.
- **Watching others:** every 3 s each node LISTs `nodes/` (one request, returns every
  lease's ETag). A lease whose ETag has not changed for 10 s **by the observer's own
  clock** is suspect. No two clocks are ever compared.
- **Tombstone:** the suspect's **guardian** (the first live node in rendezvous order for
  the suspect's id) CASes the suspect's lease, conditioned on the exact ETag it watched,
  to `state: dead`. If the suspect renewed meanwhile, the CAS fails and nothing happens.

A network split between nodes never causes a takeover: both sides still renew their leases
in the bucket, so nobody's ETag stops changing.

Leases are never deleted: incarnation numbers must never be reused (old snapshot pointers
and split markers name them).

### 8.2 Owner records: who owns a shard

Each shard has `owners/{hh}/{shard}`:

```json
{ "node": "valka-2", "inc": 7, "gen": 12, "state": "owned" }
```

It is written only when the shard moves, so a stable cluster never writes it. A shard can
be **claimed** only if its record is missing, `released`, or names an incarnation that is
**dead and whose log is split**. Every move is a CAS, so two nodes that briefly disagree
can't both win.

### 8.3 Placement: who should own a shard

Every node computes the same answer independently: **rendezvous hashing** over the
eligible nodes (alive, not draining, past a 15 s join delay, compatible protocol). The
desired owner of shard `s` is the node with the highest `hash(s, node)`. Adding or removing
a node moves only ~1/N of the shards. There is no leader.

Every 3 s each node runs a **placement round**: claim shards whose desired owner is me and
that are claimable; release shards I own whose desired owner is another node (at most 64
per round, and never within 5 minutes of the shard's last move, so a flapping node can't
make shards bounce). Operators can **pin** a shard to a node (`placement.json`, admin API);
a pin wins while that node is eligible.

### 8.4 Claim and release

**Claim** of shard `S` by node `B`:

```
1. GET owners/S                              claimable? (missing / released / dead+split)
2. CAS owners/S → {B, inc_B, gen+1, owned}   S's history now belongs to B, at gen+1
3. LIST S's snapshots, load the newest       (after the CAS: anything a split wrote is visible)
4. PUT an acquisition snapshot (gen+1) pointing into B's own log
5. serve S
```

Step 4 is what makes failover exact: from now on, the newest snapshot of `S` always points
into the log of `S`'s current owner, so whoever claims `S` next has exactly one log to
replay. A claim that crashes between steps 2 and 4 is harmless: `B` never served `S`; the
next claimer takes `gen+2` from the same snapshot. If `B` is still alive, it finishes its own
claim (the shard is "stranded" until it does).

**Release** by its owner `A` (planned move, drain, rolling restart):

```
1. stop accepting commands for S (callers get NOT_OWNER and retry at the new owner)
2. wait until every record of S is durable
3. PUT a final snapshot
4. CAS owners/S → released
5. drop S from RAM
```

**Why the own-lease check makes this safe.** Suppose `A` freezes (a long GC pause, a VM
freeze) and is declared dead while it has a segment in flight. The order of events is
forced: `A` PUTs its segment, *then* checks its own lease, *then* releases the acks. The
tombstone is a write to that lease. So either the check happened before the tombstone (and
the segment was in the bucket before the tombstone, hence before the split reads the log,
so the split sees it), or the check happened after the tombstone (and fails, so the record
is never acknowledged). A frozen node that wakes up finds itself fenced and exits; nothing
it acknowledged is lost, nothing it did not acknowledge is served.

### 8.5 Splitting a dead node's log

A dead node's log holds records of many shards. Rather than have every node that claims
one of those shards read the whole log, the guardian reads it **once** and turns it into
per-shard snapshots:

```
1. read the dead incarnation's watermark w from its (frozen) lease
2. LIST its segments from w; the end is the first missing position (or a SEALED one)
3. PUT an empty SEALED segment at that end        (a late PUT from the dead node now fails)
4. read the segments from w up to the seal
5. if splits/{node}/{inc} exists already: another split finished; stop
6. for each shard whose owner record names this incarnation (at gen g):
     newest snapshot at gen g + this log's records of (shard, g) → a split snapshot
7. PUT splits/{node}/{inc}                        "done"; its shards are now claimable
8. delete the dead log
```

- **Why the first missing position is the end.** Acks are released in segment order, so
  every acknowledged segment sits below the first hole. A segment above a hole was never
  acknowledged; sealing the hole makes its late PUT pointless, because readers stop at the
  seal.
- **Why start at the watermark.** Truncation raises the watermark before it deletes, and a
  dead incarnation can no longer raise it. So nothing at or above `w` can have been
  truncated, and holes below `w` are irrelevant.
- **Why re-check the marker after reading, and load snapshots only after reading.** Two
  splitters can race; the TLA+ model found that a slow splitter could otherwise read a log
  that GC had already deleted and refilled. With the re-check, every splitter that writes
  read the same log and writes the same snapshots (create-only PUTs keep the first).

A restarted node splits its own previous incarnation the same way, so restart and takeover
are one code path.

### 8.6 Garbage collection

The desired owner of shard 0 runs a garbage collector every 5 minutes: dead logs after
their split marker exists, old snapshots, and split markers nothing references any more.
Every delete is checked for "what if this runs arbitrarily late" (PHASE2.md §8.7). Leases
are never collected.

## 9. Requests and workers across nodes

### 9.1 Routing a request

Every REST and gRPC call goes through `Api` (`crates/valka-server/src/api.rs`):

| Request | Runs on |
|---|---|
| create task | the receiving node picks the partition and mints the id, then the shard's owner creates it |
| get / cancel / delete / signal / runs / checkpoints of a task | the owner of the shard in the task id |
| list tasks, dead letters, workers, counts, cluster stats, shard map | every live node, merged |

The owner is found from a cache, refreshed from `NOT_OWNER` replies (which carry a hint)
and, as a last resort, from the owner record in the bucket. While a shard is moving,
callers get `UNAVAILABLE` and retry (the SDKs do). A scatter-gather answer that missed a
node is marked partial (`X-Valka-Partial: true`).

Internal calls carry the cluster token (compared in constant time) and can use mTLS.

### 9.2 Matching

Each node keeps, per queue, the local workers that are waiting for a task. The owner of a
task hands it out in two ways:

1. **Local:** when the task becomes runnable (created, retried, returned) and a local
   worker is waiting, it is offered directly, as soon as its record is durable (the hot
   path).
2. **Remote:** other nodes with waiting workers long-poll the owner (`Fetch`). The owner
   reserves a runnable task for the fetching node in RAM and hands it over; the fetching
   node's worker then dispatches it through the owner, which writes `TaskDispatched` with
   the worker and the worker's node.

Either way a handed-out task is marked *offered* and leaves the pending index until its
dispatch is recorded. A refused dispatch hands it straight back, and any offer not
dispatched within 30 s (`OFFER_TTL`) lapses back to the index, so a lost hand-off can delay
a task but never strand it.

Tasks nobody is waiting for stay in the owner's pending index; nothing is pushed blindly.

### 9.3 Reattach after a reconnect

A worker that loses its node keeps running its tasks, reconnects (to another node if its
server address is a load balancer or DNS name in front of several nodes, otherwise to the
same node once it is back), and says
in its hello which runs it is still executing. For each one the owner answers:

- **Reattached:** the run is current; the owner records the worker's new node
  (`RunReattached`) so results, cancels and signals flow again.
- **Adopted:** the owner died right after dispatching and its `TaskDispatched` record was
  lost; the task is PENDING at exactly that attempt. The owner records the dispatch under
  the worker's run id instead of running the task a second time.
- **Stale:** the run is over (cancelled, expired, finished elsewhere); the worker is told
  to stop it.

After claiming shards, a node holds back recovered PENDING tasks for 5 s
(`reconcile_grace`) so reconnecting workers can adopt their runs before anything is
dispatched again.

## 10. Failure walkthroughs

| What happens | What the cluster does | What you see |
|---|---|---|
| `kill -9` on a node | Its lease stops changing; after 10 s its guardian tombstones it, splits its log (~1 s), and the desired owners claim its shards | Its shards are unavailable until claimed: 12–28 s in the [failover benchmark](BENCHMARKS.md#failover-three-nodes-in-docker-minio), where claiming dominates on a slow bucket (`UNAVAILABLE`, clients retry). Its workers reconnect elsewhere and reattach. Nothing acknowledged is lost |
| Node frozen 30 s, then wakes up | Taken over while frozen. On waking, its segment PUTs hit the seal or are ignored, its own-lease check fails, its renewal fails: it exits and restarts as a new incarnation | Requests it held fail; clients retry elsewhere |
| Node cut off from the bucket | It can't PUT, so it acknowledges nothing; after 7 s it stops serving; after 10 s it is tombstoned and its shards move | ~15 s unavailability for its shards |
| Network split between nodes (all reach the bucket) | No takeover: leases keep renewing. Forwarding across the split fails | Cross-side requests get `UNAVAILABLE` until it heals |
| Bucket down for everyone | No acks anywhere; every node stops serving; nobody can tombstone anybody | Full outage, recovers by itself |
| Two nodes claim one shard | CAS on the owner record: one wins | Nothing |
| Node restarts with the same id | New incarnation fences the old one and splits its own old log at once; peers claim its shards (alone: it claims them itself); its share moves back later | Seconds of unavailability for those shards, no lease timeout to wait out |
| `SIGTERM` (rolling restart) | If live peers exist: release every shard with a final snapshot, then exit; peers claim without any replay. Alone: keep the shards, reclaim on restart | Near-zero unavailability |
| A worker's node dies mid-task | The worker keeps running, reconnects (another node behind its address, or the same one once restarted), reattaches; its result is resent and acknowledged | The task completes once |
| The owner dies right after dispatching | The worker's reattach adopts the run | No duplicate run |
| A worker dies mid-task | The run's lease expires; the task is retried (checkpointed steps are skipped) | One retry |

## 11. What is guaranteed, and what is not

Guaranteed, under every failure above:

- **Acknowledged means durable.** A create, result, cancel, checkpoint or signal that got a
  success response is in the bucket and survives any node failure.
- **One history per shard.** Two nodes never write diverging histories of the same shard.
- **Bounded recovery.** At most 64 MiB of log per node to replay or split.
- **Failover.** A dead node's shards are served again within the lease timeout plus the
  claims (12–28 s measured), as long as some node can reach the bucket.
- **Exact idempotency.** Two creates with the same idempotency key in the same queue
  produce one task.

Not guaranteed, by design:

- **Exactly-once execution.** Execution is at-least-once: a task can run twice around
  failures (worker death, lease expiry, a lost dispatch that could not be adopted). Make
  handlers idempotent, or use steps.
- **Global ordering.** FIFO holds per queue partition (priority first); a queue with 16
  partitions has no global order. Set the queue's partitions to 1 for strict FIFO.
- **Durable reads.** Reads are served from RAM and can show a write whose acknowledgement
  is still in flight (that write's caller has not been told it succeeded, and will retry
  if it fails).
- **Low-latency acks.** An acknowledgement waits for an object-store PUT.

## 12. Timings and knobs

| Setting (`VALKA_…`) | Default | Effect |
|---|---|---|
| `WAL__FLUSH_INTERVAL_MS` | 50 | group-commit window: ack latency vs. PUT count |
| `WAL__SNAPSHOT_INTERVAL_SECS` | 60 | snapshot cadence for changed shards |
| `WAL__LOG_BUDGET_BYTES` | 64 MiB | max uncovered log per node (bounds restart / split time) |
| `CLUSTER__NUM_SHARDS` | 4096 | shard count of a **new** bucket (power of two, 16–65,536); fixed afterwards |
| `CLUSTER__LEASE_INTERVAL_MS` | 2000 | lease renewal period |
| `CLUSTER__LEASE_TIMEOUT_MS` | 10000 | unchanged lease ⇒ dead (failure detection time) |
| `CLUSTER__LEASE_SAFETY_MARGIN_MS` | 3000 | stop serving at `timeout − margin` without a renewal |
| `CLUSTER__MEMBERSHIP_INTERVAL_MS` | 3000 | how often `nodes/` is listed |
| `CLUSTER__JOIN_DELAY_MS` | 15000 | a new node takes shards after this (not on a cold start) |
| `CLUSTER__PLACEMENT_INTERVAL_MS` | 3000 | placement round period |
| `CLUSTER__MIN_RESIDENCY_SECS` | 300 | a shard stays put this long after a move |
| `CLUSTER__HANDOVER_PER_TICK` | 64 | shards released per placement round |
| `CLUSTER__RECONCILE_GRACE_MS` | 5000 | hold recovered PENDING tasks for reattaching workers |
| `CLUSTER__SPLIT_TIMEOUT_MS` | 30000 | another node may redo a split after this |
| `CLUSTER__GC_INTERVAL_SECS` | 300 | garbage collector cadence |

Cost: per node, about one lease PUT every 2 s, one own-lease GET per flush (mostly `304`),
one `nodes/` LIST every 3 s, plus the segment PUTs themselves. None of it grows with the
number of nodes.

## 13. How it is verified

| Layer | Where | What it shows |
|---|---|---|
| TLA+ model | `spec/tla/` (TLC in CI; long configurations nightly) | leases, ownership, sealing, splitting, truncation and GC keep every acknowledged record and one history per shard, under arbitrary pauses, crashes and late PUTs. It found 7 protocol bugs before any code existed (PHASE2.md §19) |
| Randomized simulation | `crates/valka-engine/src/sim_tests.rs` | 3–4 engines on one in-memory bucket with fault injection (kills, freezes, drains, lost PUT responses, bucket errors), paused time, and a history checker over every acknowledged operation. It found 4 implementation bugs (PHASE2.md §19a) |
| Multi-engine scenarios | `crates/valka-engine/src/cluster_tests.rs`, `safety_tests.rs` | every failure walkthrough above, deterministic |
| Engine and WAL | `crates/valka-engine/src/tests.rs`, `crates/valka-wal` | crash/replay property test, timers, the writer under faults |
| Integration | `crates/valka-tests` | REST, gRPC and the Rust SDK end to end; 3-node clusters over real localhost gRPC (routing, cross-node workers, reattach, admin, mTLS) |
| Real S3 | `--features minio` | conditional-write semantics, storage self-test, two-node takeover |
| Chaos | `scripts/chaos.sh` + `examples/rs/chaos.rs` | a 3-node docker compose cluster with random `kill -9`, freezes and restarts under load; checks that every acknowledged task exists and finishes and no idempotency key made two tasks. It found 3 bugs: a drain stopped renewing its lease before handing its shards over; a task whose local dispatch failed stayed "offered" forever; a create whose forwarding node died mid-request became durable but was never made runnable (PHASE2.md §19b) |

## 14. Compared with Cloudflare Durable Objects

Durable Objects (DOs) are the closest well-known relative: both put a single-threaded
owner in front of state and make durability a precondition of every response. The
comparison is based on Cloudflare's public descriptions of DOs and their SQLite storage.

| | Durable Objects | Valka |
|---|---|---|
| Unit of ownership | one object (millions of them, created on demand) | one shard (default 4096, fixed per cluster), holding many tasks |
| Who decides the owner | Cloudflare's internal placement and directory services | the owner record in the bucket, moved only by CAS |
| Single writer | guaranteed by the platform | fenced by each node's lease plus an own-lease check after every segment PUT |
| Durability before a response | "output gates": the response waits until the write is confirmed by replicas in nearby data centers; object storage is written later in batches | the response waits until the WAL segment is in the bucket |
| Ack latency | single-digit milliseconds | the object store's PUT latency plus up to the 50 ms commit window |
| Where state lives | local SQLite, replicated log, periodic object-storage copies | RAM, rebuilt from snapshots plus the WAL in the bucket |
| Failover | handled by the platform | ~10–15 s: lease timeout, tombstone, log split, claim |
| Concurrency inside the owner | one thread per object, "input gates" against interleaving | a lock per shard; every change is one record applied under it |
| What you run | nothing (managed) | `valka-server` processes plus any S3-compatible bucket |

The shared idea is the actor-plus-log pattern: route every request for a piece of state to
its one owner, keep the state in memory, and never answer before the log entry describing
the change is safe. The difference is where the log goes: DOs replicate it across a fleet
of their own machines for latency; Valka writes it to an object store and accepts
higher latency in exchange for needing nothing but a bucket.

## 15. Where the code is

| Concept | File |
|---|---|
| Shards, ids, partitions, hashing | `crates/valka-core/src/shard.rs` |
| Configuration | `crates/valka-core/src/config.rs` |
| Bucket keys | `crates/valka-wal/src/lsn.rs` (`keys`) |
| Records | `crates/valka-wal/src/record.rs` |
| Segment format | `crates/valka-wal/src/segment.rs` |
| Group-commit writer, own-lease check | `crates/valka-wal/src/writer.rs` |
| Leases, renewal, self-fencing, tombstones | `crates/valka-wal/src/lease.rs` |
| Owner records | `crates/valka-wal/src/owners.rs` |
| Snapshots | `crates/valka-wal/src/snapshot.rs` |
| Sealing, split markers | `crates/valka-wal/src/split.rs` |
| `Engine::mutate` (the write path) | `crates/valka-engine/src/write_path.rs` |
| State machine (`ShardState::apply`) | `crates/valka-engine/src/apply.rs` |
| Claim, release, split | `crates/valka-engine/src/ownership.rs` |
| Membership, placement, guardian, GC | `crates/valka-engine/src/cluster.rs` |
| Task commands, partition choice | `crates/valka-engine/src/tasks.rs` |
| Pending index, remote reservations | `crates/valka-engine/src/pending.rs` |
| Snapshot round, truncation, rebuild | `crates/valka-engine/src/recovery.rs` |
| Request routing | `crates/valka-server/src/api.rs` |
| Internal RPC commands | `crates/valka-cluster/src/rpc.rs`, `proto/valka/v1/internal.proto` |
| Peer clients, token, mTLS | `crates/valka-cluster/src/peer.rs`, `security.rs` |
| Worker streams, reattach | `crates/valka-dispatcher/src/` |
| Remote feeding | `crates/valka-server/src/feeder.rs` |
| Admin API | `crates/valka-server/src/admin.rs` |

## 16. Glossary

| Term | Meaning |
|---|---|
| **acquisition snapshot** | the snapshot a claimer writes before serving, pointing into its own log |
| **CAS** | compare-and-swap: a PUT that succeeds only if the object's ETag is still the one read (or the object is still absent) |
| **claim / release** | taking a shard over / handing it back (§8.4) |
| **desired owner** | the node rendezvous hashing picks for a shard |
| **fenced** | a process whose lease was taken (tombstone or newer incarnation); it can't acknowledge anything and exits |
| **final snapshot** | the snapshot a releasing owner writes before giving a shard up |
| **generation (`gen`)** | per shard, bumped on every ownership change; orders snapshots across owners |
| **group commit** | many records from many shards in one segment PUT |
| **guardian** | the node responsible for watching (and tombstoning) a given other node |
| **incarnation (`inc`)** | one run of a node process; bumped at every start |
| **lease** | `nodes/{node}/lease`, renewed every 2 s; liveness and the write fence |
| **log budget** | the most log a node may have that no snapshot covers (64 MiB) |
| **owner record** | `owners/{hh}/{shard}`: which incarnation owns a shard, at which generation |
| **partition** | a slice of a queue that lives on one shard |
| **pending index** | per shard, the runnable tasks not yet handed out |
| **reattach / adopt** | a reconnecting worker re-linking its running task to the owner (§9.3) |
| **seal** | an empty SEALED segment at the end of a dead log; nothing can be added past it |
| **self-fence** | a node that can't renew stops serving on its own, before anyone may take over |
| **shard** | the unit of ownership and of state; tasks never span shards |
| **split** | turning a dead incarnation's log into per-shard snapshots (§8.5) |
| **stranded shard** | a shard whose claim may have passed its CAS but did not finish; its claimer retries it |
| **tombstone** | a guardian's CAS of a silent node's lease to `dead` |
| **watermark (`trunc`)** | the lowest log position that may still exist; raised before truncating |
