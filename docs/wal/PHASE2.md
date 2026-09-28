# Phase 2: Valka on many nodes

Status: **plan, not started.** Companion to `DESIGN.md` (single-node architecture) and
`PLAN.md` (checklist). Where this document and `DESIGN.md` disagree, this one is newer;
`DESIGN.md` is rewritten as each milestone lands.

Reading guide: §1–§4 are the contract (goals, fault model, guarantees, decisions). §5–§12
are the architecture, one protocol per section, each with *why it is safe*. §13–§18 are
failure analysis, verification and the build plan.

---

## 1. Goals

- Run any number of `valka-server` nodes against one bucket. Adding a node adds capacity.
- **No acknowledged work is ever lost**, whatever fails: `kill -9`, machine loss, a frozen
  process, a network split, a slow or failing bucket.
- A dead node's work is served elsewhere within **10–15 s**.
- The bucket stays the only dependency. No etcd, no ZooKeeper, no Raft, no database.
- Safety never depends on clocks. Clocks only decide *how fast* we react to failures.
- One node is just a cluster of one: there is one code path, not a single-node special case.

Non-goals: multi-region clusters, changing the shard count after creation, load-aware
placement (phase 3), public API authentication (separate roadmap item).

## 2. Fault model

What the design must survive:

| Fault | Assumed possible |
|---|---|
| Node crash (process or machine), with or without restart | Yes |
| Arbitrarily long process pause (GC, SIGSTOP, VM freeze), then resuming as if nothing happened | Yes |
| Lost, delayed, duplicated or reordered messages between nodes | Yes |
| Network split between nodes, with every node still reaching the bucket | Yes |
| Node cut off from the bucket | Yes |
| Bucket requests failing, slow, or succeeding with the response lost | Yes |
| Wall clocks wrong by seconds | Yes (affects timer precision only) |
| Monotonic clocks running at a very different rate | No: bounded drift is assumed for **failure detection and read fencing only** |
| Bucket violating its consistency contract (§12.1) | No: checked at startup, refused if it lies |
| Malicious nodes | No: internal traffic is authenticated (§11.6), nodes are trusted |

## 3. Guarantees

| # | Guarantee | Plain meaning |
|---|---|---|
| G1 | **Acked means durable.** Every acknowledged mutation is reflected in the state of every future owner of its shard. | "Saved" is forever. |
| G2 | **One history per shard.** A shard's committed records form a single sequence; no two owners ever produce diverging histories. | Never two writers. |
| G3 | **Bounded recovery.** Uncovered WAL per node is capped (log budget), so takeover work is bounded. | Recovery can't take forever. |
| G4 | **At-least-once execution.** Every task runs to a terminal state at least once; duplicates happen only around failures and are bounded by the reattach protocol. | Same promise as SQS / Temporal activities. |
| G5 | **Liveness.** If a node dies, its shards are served by live nodes after detection (default ≤ 15 s p99), provided a live node can reach the bucket. | Failover actually happens. |

G1 and G2 hold under every fault in §2. G5 needs the bounded-drift assumption. The TLA+
model (§15.1) checks G1 and G2 exhaustively for small clusters.

## 4. Decisions

| # | Decision | Chosen | Why | Owner |
|---|---|---|---|---|
| D1 | Task routing across nodes | **Node-to-node streams**, with two-phase slot claims (§10) | One ownership domain, no SDK fan-out | You |
| D2 | Cluster size | **Unbounded by design**; the ceiling is `num_shards / 8` useful nodes | No shared hot object; per-node bucket traffic is O(1) | You |
| D3 | Failover target | **10–15 s** (lease timeout 10 s) | Few false takeovers, cheap | You |
| D4 | Verification | **TLA+ model + deterministic simulation + chaos** | Protocol bugs found before code | You |
| D5 | Task placement | **Queue partitions** (§9) | Cross-node traffic bounded at any size; FIFO per partition | You |
| D6 | Shard count | **Chosen at cluster creation**, 256–65,536, stored in `cluster.json` | Large clusters possible without a migration | You |
| D7 | Deploy targets | **Kubernetes and VMs equally** | | You |
| D8 | Node security | **Cluster token always, mTLS optional (recommended)** | Safe by default, easy to deploy | Delegated to me |
| D9 | Shard ownership records | **One object per shard**, CAS'd only when the shard moves | No single hot object | Mine |
| D10 | Write fencing | **Each flush checks the node's own lease** (1 conditional GET) | O(1) per node at any cluster size; needs no clock | Mine |
| D11 | Snapshot and record ordering | **Per-shard ownership generation** (`gen`), bumped on every move | Orders snapshots across owners; per-node epochs cannot (see §19) | Mine |
| D12 | Takeover replay | **Log splitting**: one node splits a dead node's log into per-shard snapshots | Each dead log is read once, not once per claiming node | Mine |
| D13 | Membership | **Gossip for discovery and suspicion, bucket for authority** | Gossip scales; the bucket decides | Mine |
| D14 | Placement | **Leaderless rendezvous hashing** plus CAS | No leader to fail over | Mine |
| D15 | Local-disk backend | **Single node only**; clustered mode refuses it | Its CAS is an in-process lock | Mine |

"Mine" decisions follow from yours plus the reliability goal; each is argued in its section.

---

## 5. Architecture

```
                     producers / API clients                 workers (any SDK)
                              │ REST, gRPC                        │ one Session stream each
                              ▼                                   ▼
          ┌──────────────── any node ────────────────────────────────────────────┐
          │  API layer ──routes by shard──►  local engine  or  forward to owner  │
          │  Dispatcher: worker streams, capacity offers, result forwarding       │
          └───────┬──────────────────────────────────────────────┬───────────────┘
                  │ internal gRPC (token/mTLS)                   │
     ┌────────────┴──────────┐   peer streams (offers, claims,   ┌┴──────────────────────┐
     │ node A                │◄──assignments, forwarded calls)──►│ node B                │
     │  shards it owns:      │                                   │  shards it owns:      │
     │   state in RAM        │   gossip (chitchat): membership,  │   state in RAM        │
     │   WAL writer (own log)│◄──suspicion, addresses, version──►│   WAL writer (own log)│
     └───────────┬───────────┘                                   └───────────┬───────────┘
                 │                                                           │
                 ▼                                                           ▼
  ┌───────────────────────────── bucket: the only truth ─────────────────────────────────┐
  │ cluster.json            created once: cluster id, num_shards, format version         │
  │ nodes/{node}/lease      liveness + fencing token (incarnation, state), CAS'd          │
  │ owners/{hh}/{shard}     who owns the shard + generation, CAS'd only on moves          │
  │ queues/{hh}/{queue}     partition count, CAS'd on admin change                        │
  │ wal/{node}/{inc}-{seq}.seg          per-node log, create-if-absent, sealable          │
  │ snapshots/{hh}/{shard}/{gen}-{seq}-{ptr}.snap   per-shard state, ordered by (gen,seq) │
  │ splits/{node}/{inc}     "this dead incarnation's log is fully split" marker          │
  │ logs/{hh}/{run}/…       task log chunks (not part of the WAL)                        │
  └──────────────────────────────────────────────────────────────────────────────────────┘
```

`{hh}` is two hex digits of a hash of the rest of the key. It spreads high-rate key
families over S3 partitions, so a takeover storm doesn't hit one prefix's request limit.

Vocabulary:
- **incarnation** (`inc`): one run of a node process. Bumped on every start; it fences the
  previous run of the same node id.
- **generation** (`gen`): per shard; bumped on every ownership change.
- **seal**: an empty segment marked SEALED, written into a dead incarnation's log so nothing
  can be added past it.
- **split**: turning a dead incarnation's log into per-shard snapshots.

## 6. Storage format changes (before any release)

`wal` is unreleased, so all of these land without a migration path. Existing dev buckets
are wiped.

| Item | Phase 1 | Phase 2 |
|---|---|---|
| Shard id in task/run ids | low 12 bits of UUIDv7 `rand_b` | low **16** bits; `shard = bits & (num_shards − 1)` |
| Shard count | const 4096 | `cluster.json.num_shards`, power of two, 256–65,536, default 4096 |
| Log position | `(epoch, seq)` | `(inc, seq)` (renamed: it was always the incarnation) |
| Record envelope | shard, shard_seq, record_id, ts | + **`gen`**: the owning generation that wrote it |
| Segment header | epoch, seq, node hash | inc, seq, node id, **flags (SEALED)**, **sorted shard list** |
| Snapshot key | `snapshots/{shard}/{lsn}.snap` | `snapshots/{hh}/{shard}/{gen}-{shard_seq}-{ptr}.snap`, where `ptr` = `{node}.{inc}.{seq}`, `final` or `split` |
| Ownership | one `assignment` object | `owners/{hh}/{shard}` per shard + `nodes/{node}/lease` |
| Queues | implicit | `queues/{hh}/{queue}` with partition count |

The snapshot's log pointer is in its **key**, so garbage collection can run on LIST alone.

## 7. Node lifecycle and leases

### 7.1 Lease object

`nodes/{node}/lease` holds
`{inc, state: alive|draining|dead, counter, internal_addr, api_addr, version, protocol, dead_by?}`.
Only its owner writes it, except for exactly one kind of write by another node: the
tombstone (§7.4). Every write is a CAS (`If-Match`).

### 7.2 Start

```
1. read or create cluster.json; refuse to start if num_shards / format differ from config
2. storage self-test (§12.1); refuse clustered mode on local/memory backends
3. read nodes/{me}/lease; CAS it to {inc: old.inc + 1, state: alive}        ← fences my previous run
4. if my previous incarnation owned shards: split its log (§8.3)         ← as any dead node's
5. join gossip (seeds from config, else from the addresses in nodes/*/lease)
6. wait join_delay, then take part in placement (§8.1)
```
Step 3 is a CAS, so two processes started with the same node id cannot both succeed.
The loser exits.

### 7.3 Renewal and self-fencing

- Every `lease_interval` (2 s) the node CAS-bumps `counter`. A conflict is resolved by
  GETting the lease. If it's still mine and alive, my earlier write landed and only its
  response was lost, so I retry. Anything else means I am fenced: **poison and exit**.
- The node records the monotonic time it *sent* each renewal that then succeeded. If more
  than `lease_timeout − safety_margin` (10 s − 3 s) passes without one, the node
  **self-fences**: it stops dispatching tasks, stops serving reads for its shards, and
  answers `Unavailable`. It resumes after the next successful renewal. A node that can't
  reach the bucket therefore goes quiet on its own *before* anyone can take over.

### 7.4 Death detection and tombstone

1. **Suspicion.** Gossip's failure detector flags a peer. A backstop LIST of `nodes/`
   (every 30 s) catches anything gossip misses.
2. **Confirmation.** The suspect's *guardian* polls its lease every `lease_interval`. The
   guardian is the first live node in rendezvous order for the suspect's id; any live node
   takes over this job after `2 × lease_timeout` if the guardian itself fails. The suspect is
   dead once its ETag and counter have not changed for `lease_timeout` **by the guardian's
   own monotonic clock**. No two clocks are ever compared.
3. **Tombstone.** The guardian CASes the lease, conditioned on the unchanged ETag it
   observed, to `{state: dead, dead_by}`. If the CAS fails, the suspect renewed and is alive,
   so suspicion is cleared.
4. The guardian then splits the dead incarnation's log (§8.3).

*Why a network split between nodes never causes a takeover:* takeover needs the lease to
stop changing in the bucket. A node that still reaches the bucket keeps renewing.

## 8. Shard ownership

### 8.1 Placement

- `desired_owner(shard)` is the highest `hash(shard, node)` over **eligible** nodes:
  alive, not draining, alive for at least `join_delay`, and on a compatible protocol
  version. This is rendezvous hashing: adding or removing one node moves only about 1/N of
  the shards, and every node computes the same answer from the same membership.
- A **placement loop** runs every `placement_interval` (3 s, jittered) and makes two kinds
  of move:
  - **Claim** (§8.4): shards whose desired owner is me and which are released, unowned, or
    owned by a dead incarnation whose split is done. Batched, at most `claim_concurrency`
    at once.
  - **Release** (§8.5): shards I own whose desired owner is another eligible node. Limited to
    `handover_rate`, and never within `min_residency` (5 min) of the shard's last move, so a
    flapping node can't make shards bounce.
- Nodes can briefly disagree about membership. That is safe because every move is a CAS on
  `owners/…`; they converge within a few ticks.

### 8.2 Owner record

`owners/{hh}/{shard}` holds `{node, inc, gen, state: owned|released}`. It is written only
by claim and release, so for a stable cluster it is never written.

### 8.3 Log splitting (dead incarnation `D = (node, inc)`)

Run by D's guardian. For a restarted node, it is run by the node's own next incarnation.
Any node may run it again after `split_timeout`; the result is idempotent.

```
1. LIST wal/{node}/{inc}-*; find the contiguous run of segments and its first gap
   (or an existing SEALED segment, which is then the end)
2. PUT-create a SEALED empty segment at the first gap
   (if that position was just filled by a late PUT: LIST again, retry at the new gap)
3. read segments up to the seal (range GETs, in parallel), group records by (shard, gen)
4. for each (shard, gen): load that shard's latest snapshot at that gen, apply records
   with shard_seq > snapshot.seq in log order, PUT-create a snapshot
   (gen, last_seq, ptr = split)
5. PUT-create splits/{node}/{inc}                                         ← "split done"
6. delete D's segments (garbage collector backstop: §8.7)
```

Why it is safe:
- **Nothing acked is missed (G1).** A segment was acked only after its own-lease check
  (§12.3) passed. That check happened before the tombstone, which happened before this
  LIST. The segment's PUT happened before the check, and bucket LIST is strongly consistent,
  so step 1 sees it.
- **Nothing is added behind the seal.** A dead node's in-flight PUTs can land out of order.
  A segment after a gap was never acked, because acks resolve strictly in order. Sealing the
  gap makes the missing PUT fail, and every reader stops at the seal. Positions below the gap
  already exist and are create-if-absent, so they can't be overwritten.
- **Deterministic.** Two splitters running at once write semantically identical snapshots;
  create-if-absent keeps the first.
- **Old generations are harmless.** If D owned a shard, released it and later re-claimed it,
  its log holds records of both generations. A split snapshot of the older generation sorts
  below the newer one and is never loaded.

A dead log is read **once**, by one node, instead of once per node that claims one of its
shards. The log budget (§12.5) bounds how much there is to read.

### 8.4 Claim (shard `S`, by the desired owner `B`)

```
1. GET owners/S → {owner, gen g, etag}
   allowed if: released, unowned, or owner's incarnation is dead AND splits/… exists
2. CAS owners/S → {B, inc_B, gen g+1, owned}                           ← S's history moves to B
3. load S's latest snapshot by (gen, seq); its ptr must be split, final, or point into a
   dead-and-split incarnation; anything else is a bug → refuse, alert
   (no snapshot at all is valid only for a never-owned shard, gen 0: start empty)
4. PUT acquisition snapshot (gen g+1, seq, ptr = B.inc_B.next_seq)       ← BEFORE serving
5. mark S Serving; hold re-dispatch of recovered PENDING tasks for reconcile_grace (§11.4)
```

Why step 4 must come before serving: B's records carry `gen g+1`. If B crashed with no
snapshot at `g+1`, the next claimer would load the `gen g` snapshot, whose pointer says
"nothing more", and miss B's records. With the acquisition snapshot in place, there is
exactly **one** log to replay for any shard: the one its latest snapshot points to. That
log is always the current owner's (G2, G3).

A crash between steps 2 and 4 is harmless. B never served S, so B's log has no records for
S, and the next claimer CASes `g+2` and loads the same `gen ≤ g` snapshot.

### 8.5 Release (graceful handover by live owner `A`)

```
1. S stops accepting commands: NotOwner {hint: desired owner}; callers retry there
2. return S's tasks from matching buffers; wait until every record of S is durable
3. PUT final snapshot (gen g, seq, ptr = final)
4. CAS owners/S → {released, gen g}
5. drop S's state, timers, pending index
```
A crash between steps 3 and 4 leaves the owner as A. A's lease dies, the split finds no S
records after the final snapshot, and the claim proceeds normally.

### 8.6 Replay (own shards after restart, or while splitting)

Apply a record only if its `shard` is the one being rebuilt, its `gen` equals the base
snapshot's `gen`, and its `shard_seq` is greater than the snapshot's. Stop at a SEALED
segment or at the first gap. The `gen` filter is a second line of defence: a record from any
other generation is skipped and alerted, never applied.

### 8.7 Truncation and garbage collection

Every delete is audited for *what if it runs arbitrarily late* (a zombie):

| Delete | Rule | Safe if run late because… |
|---|---|---|
| Own log truncation | below the minimum log pointer over my Serving shards' latest durable snapshots | every snapshot a claimer could load points at or above that bound |
| Snapshot prune | keep the newest 2 per shard by (gen, seq); never the newest | the newest is never deleted |
| Dead-incarnation log | only after `splits/{node}/{inc}` exists | split snapshots cover everything in it |
| Split markers | after the incarnation's log is gone and no snapshot key references it | nothing reads it |

A garbage collector (the guardian of shard 0) LISTs `wal/`, `snapshots/` and `splits/`
every 5 minutes and applies the same rules. It is the backstop for crashed deleters.

## 9. Queue partitions (D5)

- `queues/{hh}/{queue}` = `{partitions: P, previous: {P, changed_at}?, version}`. It is
  created on first use with `default_partitions` (16) via create-if-absent, and changed only
  by an admin call.
- Partition `p` of queue `Q` lives on shard `hash(Q, p) mod num_shards`. All of a
  partition's tasks share one shard, so one owner and one matching queue serve it.
- **Choosing the partition on create:**
  - with an idempotency or routing key: `jump_hash(key, P)`. When P grows, jump hashing
    moves only about 1/P of the keys.
  - without a key: a partition whose shard this node owns if there is one (no hop),
    otherwise a random partition, forwarded to its owner.
- **Idempotency across a change of P:** while `previous` is within the dedupe TTL, a keyed
  create also checks the key's old partition. Dedupe stays exact through a resize.
- **Growing P** is online: new partitions start receiving tasks, old ones keep draining.
  **Shrinking** marks partitions draining: they take no new tasks but are served until empty.
- **Ordering:** FIFO per partition, priority order within a partition. `P = 1` gives strict
  FIFO for the whole queue.
- **Sizing guidance:** a queue's peak throughput is roughly `P × per-shard throughput`, and
  at most P nodes store it. Set `P ≥` the number of nodes you want serving the queue.
  Automatic P scaling is phase 3.

## 10. Cross-node matching (D1)

```
worker w7 ─ Session ─ node W                         node O (owns partition shard of task T)
                      │ w7 idle, serves queue Q
                      │── Offer{Q, idle: 3} ───────► O remembers: W can take Q work
                                                     T becomes runnable, no local worker free
                      ◄── Claim{Q, T} ────────────── (in memory, no WAL write)
                      │ reserve w7 atomically
                      │── Granted{w7} ─────────────►
                                                     O records TaskDispatched{w7, node W}
                      ◄── Assignment{T, run, checkpoints}
                      │ push to w7
```

- **Streams.** A node W opens one internal stream per peer that owns partitions of the
  queues W's workers serve. That's at most `Σ P` over those queues, independent of cluster
  size.
- **Offers** are counts, non-exclusive, and refreshed as slots free up. **Claims** are
  exclusive: W, which owns the workers, arbitrates in memory. A WAL write happens only after
  `Granted`, so contention between owners costs RPCs, never records.
- **Local first.** If O has an idle local worker, the existing sync-match hot path runs
  unchanged.
- **Races.** `Granted` but no `Assignment` within `claim_timeout` (O died): W frees the
  slot. An `Assignment` arriving for a slot W already freed, or for a worker that vanished:
  W answers `Returned`, and O writes a new `RunReturned` record, putting the task back to
  PENDING **without spending an attempt**. Today that case waits for lease expiry.
- **Fairness.** O rotates between the Ws that offered; W grants in arrival order.

### 10.1 Everything that crosses nodes after dispatch

| From worker (via its node W) | Goes to | Delivery |
|---|---|---|
| Result (complete / fail) | owner of the task's shard | **acked end to end** (§11.3) |
| Heartbeat | each owner of the worker's running tasks | batched per owner per tick |
| Checkpoint | owner | unary RPC, acked once durable (existing) |
| Logs | written by W's log ingester straight to the bucket | unchanged |

| From owner O | Goes to | Delivery |
|---|---|---|
| Cancel, signal | the node recorded on the run (W) → worker | retried until W acks; on reattach they are re-sent |

## 11. Requests, results and workers

### 11.1 Routing table

| Request | Handled by |
|---|---|
| Create task | partition owner (§9); local when this node owns the chosen partition |
| Get / cancel / delete / signal / runs / checkpoints by id | owner of the shard in the id |
| List tasks / counts / dead letters with a queue filter | owners of that queue's partitions, merged |
| The same without a queue filter | every node, merged (cursor pagination per node; no deep OFFSET) |
| Cluster stats and shard map | every node, merged |

Owner lookup uses `desired_owner` plus a small cache of actual owners learned from
`NotOwner{owner}` redirects and from gossip. A forwarded call that lands on a non-owner is
redirected at most twice, then fails with `Unavailable` and the caller retries.

### 11.2 Internal RPCs

These replace the PG-era `ForwardTask`, `ForwardEvent`, hash ring and partitions:
- `Execute(command)`: id-routed commands (get, cancel, delete, signal, checkpoint, result,
  heartbeat batch, reattach)
- `CreateTask(id, request)`: the id is pre-generated for the chosen shard
- `Scatter(query)`: list, count, stats
- `Peer` bidi stream: offers, claims, grants, assignments, returns, cancel and signal
  delivery, split-done notifications

### 11.3 Result delivery (end to end)

- The worker SDK keeps each result until it gets `ResultAck`. It resends on reconnect.
- W forwards the result to the current owner with backoff, re-resolving the owner on every
  retry (the shard may have moved).
- Completion becomes **idempotent**. Completing an already-completed run with the same
  run id returns success. A result for a stale run (cancelled, lease expired, another run
  won) returns a definitive `Stale`, and the SDK stops retrying.
- **Done in M0** for a single node: `ResultAck`, idempotent `Engine::report_result`, and
  resending in all four SDKs (DESIGN.md §17). Phase 2 adds the forwarding to the owner.

### 11.4 Worker reattach

- `WorkerHello` gains `running: [{task_id, run_id, attempt}]` and re-sends unacked results.
- W forwards each entry to the owner:
  - run known and RUNNING → record the worker's new node (`RunReattached`, asynchronous)
  - run unknown, task PENDING, `attempt = attempts + 1` → its dispatch record was lost in
    a crash: **adopt** it with a normal `TaskDispatched` carrying the worker's run id
  - task terminal or cancelled, or the run is stale → `Stale`; the worker cancels its copy
- After claiming shards, the owner holds re-dispatch of their recovered PENDING tasks for
  `reconcile_grace` (5 s) so reconnecting workers claim their runs first.

### 11.5 Events

- Per-task and per-queue subscriptions are routed to the owners involved, so they scale.
- The cluster-wide firehose (`/api/v1/events` with no filter) fans in from every node. It is
  supported up to about 50 nodes; beyond that, tail the WAL from the bucket (CDC).

### 11.6 Internal security (D8)

- A separate internal listener (`cluster.internal_addr`, default `:50052`). Never expose it
  publicly.
- Every internal call carries `cluster.token` (compared in constant time). Clustered mode
  refuses to start without a token.
- Optional mTLS (`cluster.tls.{cert,key,ca}`, rustls). With mTLS the token is still
  required, as defence in depth. Recommended for production; the Helm chart supports
  cert-manager.

## 12. Correctness mechanisms in detail

### 12.1 Bucket contract and startup self-test

Required: create-if-absent PUT, If-Match PUT, strong read-after-write, strong LIST, stable
ETags, range GET. Known good: AWS S3, GCS, Azure Blob, MinIO (recent), Cloudflare R2 (to be
confirmed by the self-test). At startup the node writes a probe key twice (the second
write must fail), CASes with a wrong ETag (must fail), and checks that LIST returns the
probe immediately. If any check fails, clustered mode refuses to start.

### 12.2 Writer: segment PUT

- First attempt answers `AlreadyExists`: the position was sealed. **Fenced** → poison.
- A retry answers `AlreadyExists`: GET the object and compare its content checksum with
  ours. Equal means our earlier attempt landed; different means the position was sealed
  (**fenced**).
  *This fixes a phase-1 assumption:* today any `AlreadyExists` after a retry is treated as
  "ours", which would ack a record that was never stored if a seal had taken the position.

### 12.3 Writer: fencing check before releasing acks (D10)

After a segment's PUT, before releasing its acks: a conditional GET of my own lease.

- 304, or content that is mine and alive → release the acks, in segment order.
- Anything else (tombstoned, or a newer incarnation) → **fenced**: fail these acks, poison,
  exit.

This costs one request per flush, the same as today's assignment check, whatever the
cluster size. **It uses no clock**: the ordering is PUT, then check, then tombstone, then
the split's LIST.

### 12.4 Snapshots only cover durable records

*Phase-1 bug fix.* Today the snapshot round writes a snapshot even when the WAL sync before
it fails. It also skips the sync entirely when the durable LSN looks close enough, while
records destined for the next segment are still in the buffer. Either way a snapshot can
contain records that never became durable. The fix: capture the shard state, then run an
**unconditional** `sync()`. If it fails, abort the whole round without writing anything.
With this fix, even a zombie's snapshot covers only a prefix of the true history. Such a
snapshot either ties with the split snapshot (same state) or sorts below it.

### 12.5 Log budget (G3)

Snapshot a node's dirty shards when its uncovered log exceeds `log_budget` (64 MB) or
`snapshot_interval` (60 s). This bounds both split time and restart replay. At typical S3
throughput a 64 MB split takes about a second.

### 12.6 Dispatch and read fencing

A node dispatches tasks and serves reads for a shard only while it is **not self-fenced**
(§7.3), i.e. while it has renewed recently enough that nobody can have taken over. This
keeps duplicate dispatches during a takeover to the reattach window rather than open-ended.
It relies on bounded clock-rate drift, but it affects duplicates and staleness only, never
G1 or G2.

## 13. Failure walkthroughs

| Scenario | What happens | Visible effect | Guarantees |
|---|---|---|---|
| `kill -9` node A | Guardian: lease unchanged 10 s → tombstone → split (~1 s) → desired owners claim → serve | A's shards unavailable ~11–15 s; A's workers reconnect to other nodes and reattach | G1 G2 G5 |
| A frozen 30 s (SIGSTOP / GC / VM freeze), then resumes | Taken over during the freeze. On resume: in-flight PUTs hit the seal or land above it; the own-lease check fails; renewal fails → poison, exit, restart as a new incarnation | A's unacked requests fail; clients retry elsewhere | G1 G2 |
| A cut off from the bucket | Can't PUT, so no acks; self-fences at 7 s; tombstoned at 10 s; shards move | A's shards unavailable ~15 s | G1 G2 G5 |
| Network split between nodes, bucket reachable | Leases keep renewing, so no takeover. Forwarding across the split fails; each side serves the shards it owns | Cross-side requests get `Unavailable` until it heals | G1 G2 |
| Bucket down for everyone | No acks anywhere; all nodes self-fence; nobody can tombstone anybody | Full outage; recovers by itself when the bucket returns | G1 G2 |
| Two nodes claim one shard at once | CAS on `owners/S`: exactly one wins; the loser reloads | None | G2 |
| Crash in the middle of a claim | New owner never served; next claimer takes `gen + 2` | Slightly longer unavailability for that shard | G1 G2 |
| Crash in the middle of a split | Another node re-runs it after `split_timeout`; idempotent | Takeover delayed by `split_timeout` | G1 G5 |
| Crash in the middle of a release | Final snapshot exists → the split finds nothing more → normal claim | Shard moves ~10 s later than planned | G1 G2 |
| Late PUT from a dead incarnation | Position sealed → create fails; above the seal → ignored and later garbage-collected | None | G2 |
| Same node id started twice | Lease CAS at start: one wins, the other exits | None | G2 |
| Node joins | After 15 s, ~1/N of the shards move by release + claim, rate-limited | Brief `NotOwner` retries per moved shard | — |
| Rolling restart (drain) | SIGTERM → release all shards (final snapshots) → mark lease draining → exit; the restart claims its share | Near-zero unavailability, nothing to replay | G1 G2 |
| Availability-zone loss (a third of the nodes) | Tombstones in parallel; splits in parallel; claims rate-limited across hashed prefixes | Those shards back within ~15–30 s | G1 G2 G5 |
| Worker's node dies mid-task | The worker keeps running, reconnects to another node, reattaches; its result is re-sent and acked | None; the task completes once | G1 G4 |
| Owner dies right after dispatching (dispatch record lost) | Reattach adopts the run instead of re-dispatching | No duplicate run | G4 |

## 14. Rolling upgrades and versions

- Each lease advertises `version` and `protocol`. The cluster's **effective protocol**
  is the minimum over live nodes.
- New record types, RPCs and fields that change behaviour are gated on the effective
  protocol. A node refuses to start if it would misread a record, and never applies an
  unknown record type.
- Upgrade: drain and restart one node at a time. Downgrade is possible while the effective
  protocol hasn't moved.

## 15. Verification

### 15.1 TLA+ model (before protocol code)

`spec/tla/Ownership.tla`, model-checked with TLC in CI.
- **State:** the bucket (leases, owner records, per-incarnation segments with positions,
  snapshots with (gen, seq, ptr), split markers); per node: incarnation, owned shards, writer
  buffer, in-flight PUTs, acked set.
- **Actions:** start, renew, pause/resume, crash, tombstone, split, seal, claim, release,
  append, PUT (landing late or out of order), own-lease check, ack, snapshot, truncate, GC.
- **Invariants:** `AckedDurable` (G1); `LinearHistory` (G2: no two different records at one
  `(shard, shard_seq)` in any reconstructable history); `NoLiveLogSealed`; `GCSafety`
  (nothing a possible future replay needs is ever deleted).
- **Liveness** under weak fairness: a dead node's shards are eventually served.
- **Size:** 3 nodes, 2 shards, 2 incarnations each, a few records. Protocol bugs show up at
  small sizes.

### 15.2 Deterministic multi-engine tests

2–5 engines on one shared in-memory store behind `FaultyStore`, with paused tokio time.
Most of M2–M7 is tested here: fast, reproducible, no network.

### 15.3 Simulation (turmoil)

Simulated hosts and links for gossip and internal gRPC. A store shim adds latency, errors,
lost PUT responses and reordered completions. There are scripted scenarios (every row of
§13) plus randomized fault schedules. Every run records the history of acked operations and
checks it against G1, G2 and a per-task linearizable state machine. Seeds are
reproducible. CI runs hundreds per commit and thousands nightly.

### 15.4 Chaos on real processes

docker-compose with 5 nodes, MinIO and toxiproxy. Faults: random `kill -9`, SIGSTOP pauses,
iptables network splits, and bucket slowdowns and errors. The load generator records a
history, and the same checker runs over it. A 24 h soak runs nightly. This rig is also the
blog's `kill -9` demo.

### 15.5 Release gates for phase 2

- TLC clean at the model sizes above.
- 10,000 simulation seeds clean.
- 1,000 chaos `kill -9` events with zero G1/G2 violations.
- Failover p99 ≤ 15 s, measured.
- A rolling upgrade under load with zero acked loss.
- Every §13 row has a test.

## 16. Cost and scale

Coordination overhead per node, steady state (defaults): 0.5 lease PUT/s, one own-lease
GET per flush (mostly 304s; phase 1 already does one assignment GET per flush), one
`nodes/` LIST per 30 s. Nothing grows with cluster size. WAL segment PUTs come on top and
are unchanged from phase 1.

| Nodes | Coordination requests / s | ≈ S3 Standard cost / day | Notes |
|---|---|---|---|
| 10 | ~210 | ~$10 | dominated by per-flush checks at 20 flushes/s |
| 100 | ~2,100 | ~$100 | |
| 1,000 | ~21,000 | ~$1,000 | needs `num_shards ≥ 8,192`; gossip state is O(N) |

A takeover costs one CAS, a LIST and a range read of at most `log_budget` for the dead log,
plus per shard one GET and one or two snapshot PUTs.

Scale ceilings, stated honestly:
- Useful nodes ≈ `num_shards / 8`.
- A queue partition's throughput is bounded by one shard (one owner, one lock).
- The unfiltered event firehose tops out around 50 nodes.

## 17. Build plan

Each milestone ends with its tests green and `DESIGN.md` updated. Estimates are for one
engineer familiar with the code. Nothing here is rushed: M1's model gates everything after
it.

| # | Milestone | Contents | Done when | Est. |
|---|---|---|---|---|
| **M0** | **Phase-1 hardening** (ships now, independent) — **done** | Snapshot durability fix (§12.4); `AlreadyExists` read-back (§12.2); idempotent completion + `ResultAck` + SDK result retry in all four SDKs (§11.3); SDK must not block its receive loop while waiting for a slot; exit on writer poison (today only `/healthz` reports it); log budget (§12.5) | New regression tests for each; crash/replay proptest extended with sync failures | 1.5 wk |
| **M1** | **Protocol model** | TLA+ spec (§15.1); TLC in CI; this document revised with anything the model finds | All invariants hold at the model sizes; spec reviewed | 2 wk |
| **M2** | **Formats** (§6) | `cluster.json`; 16-bit shard ids; `gen` in envelopes; segment flags and shard list; new snapshot keys; `owners/` and `nodes/` objects; hashed prefixes; storage self-test; clustered mode refuses local/memory | One node runs on the new layout; all existing tests pass | 1.5 wk |
| **M3** | **One node, full protocol** | Incarnation start (§7.2); lease renew and self-fence (§7.3); own-lease fencing check (§12.3); engine per-shard lifecycle (Unowned → Loading → Serving → Releasing); claim, release, split and replay on N = 1; restart = split own previous incarnation + claim | Multi-engine tests: restarts, released and re-claimed shards, crash at every step of claim, release and split | 2.5 wk |
| **M4** | **Membership, placement, takeover** | Gossip + guardian + tombstone (§7.4); rendezvous placement loop (§8.1); claim concurrency and rate limits; GC (§8.7); **turmoil harness begins** | 3–5 engines: kill, pause and cut off nodes; every acked task present; §13 rows 1–10 as tests | 3 wk |
| **M5** | **Queue partitions and routing** | Queue config (§9); partition choice with jump hash; resize with exact dedupe; internal RPC service with token and mTLS (§11.2, §11.6); id routing and redirects; scatter-gather with cursors; delete ring, partitions and `ForwardTask` | REST and gRPC suites pass through any node of a 3-node cluster | 2 wk |
| **M6** | **Cross-node matching** | Peer streams; offers, claims and grants (§10); `RunReturned`; result, heartbeat and checkpoint forwarding; cancel and signal routing; per-queue event routing | e2e: workers on B run tasks stored on A and C; cancels and signals reach them; results survive owner failover | 3 wk |
| **M7** | **Worker reattach** | `WorkerHello.running` + result re-send in the proto and all four SDKs (official `generate_proto.sh` only); adopt and stale handling; `reconcile_grace` | Kill a worker's node mid-task: completes once; kill an owner right after dispatch: no duplicate | 1.5 wk |
| **M8** | **Operations** | SIGTERM drain; `min_residency`; admin API (drain node, move shard, set partitions); version gates (§14); metrics (takeovers, lease age, split duration, claims, redirects, fenced events) and alert rules; runbooks (node loss, bucket outage, stuck split) | Rolling restart of 5 nodes under load: zero acked loss, only retried `NotOwner`s | 2 wk |
| **M9** | **Prove it** | Complete simulation suite (every §13 row, randomized schedules); chaos rig + history checker; 24 h soak; fix everything found | Release gates (§15.5) all green | 3 wk |
| **M10** | **Ship it** | Helm (StatefulSet, preStop drain, PodDisruptionBudget, anti-affinity, cert-manager option); systemd units and a docker-compose cluster; cluster UI phase-2 half (`CLUSTER_UI.md`); docs | A new user starts a 3-node cluster from the README on Kubernetes or on VMs, and watches a takeover in the UI | 2 wk |

**Total ≈ 24 weeks** for one engineer. M0 is valuable on its own and should ship first.
The simulation harness grows from M4 onward. M9 is when it becomes the gate.

Dependencies: M0 → M1 → M2 → M3 → M4 → {M5 → M6 → M7} → M8 → M9 → M10.

## 18. Risks

| Risk | Mitigation |
|---|---|
| An S3-compatible store that claims but doesn't honour conditional writes or strong LIST | Startup self-test (§12.1); documented support matrix; the MinIO suite in CI |
| Split takes too long under heavy write load | Log budget bounds it; split duration metric and alert; parallel range GETs |
| Hot partition limited by one shard | Documented P sizing; per-partition throughput metric; automatic P scaling in phase 3 |
| Protocol bug that the tests miss | TLA+ first; simulation with history checking; chaos; release gates |
| Gossip scaling past ~1,000 nodes | Gossip only drives suspicion; the bucket is authoritative; guardian polling is O(1) per suspect |
| Request cost at very large N | Mostly per-flush checks; can be relaxed to one check per group of flushes, still in order |
| Operational complexity | One code path for N = 1 and N = many; runbooks; UI shows every takeover |

## 19. What changed from the first draft

- **Per-node epochs → per-shard generations.** A node that restarted recently can have a
  higher epoch than the node that took over from it, so epochs cannot order snapshots of
  one shard written by different owners. Generations can (this is what Temporal's
  per-shard `range_id` does).
- **One `assignment` object → one owner record per shard plus a per-flush check of the
  node's own lease.** A single object doesn't survive large clusters; the own-lease check
  is O(1) per node and needs no clock.
- **Every claimer replaying the dead log → log splitting.** Without splitting, a dead log is
  read once per claiming node.
- **Spread over all shards → queue partitions.** Otherwise every node must exchange
  capacity with every other node.
- **Three phase-1 bugs** (§12.2, §12.4, §11.3) are fixed in M0, before any cluster work.

## 20. Open questions

- Automatic partition scaling and load-aware placement: phase 3.
- Read-your-writes across a claim for list endpoints: reads served from RAM may include
  writes still in flight. The same as phase 1, documented; a "durable reads" option is
  possible later.
- Cloudflare R2's conditional-write and LIST semantics: confirm with the self-test before
  listing it as supported.
- `ack=fast` and payload blobs: independent of this plan; they fit after M5.
