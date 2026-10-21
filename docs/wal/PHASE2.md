# Phase 2: Valka on many nodes

Status: **M0–M8 implemented**; M9 done except the 24 h soak; M10 done for docker compose and
docs (Helm and systemd are out of the current scope). The TLA+ model runs in CI. Companion
to `DESIGN.md` (single-node architecture), `PLAN.md` (checklist) and the readable overview
[`../ARCHITECTURE.md`](../ARCHITECTURE.md). Sections that were designed one way and built
another say so; §19a lists every such change.

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
| D1 | Task routing across nodes | **Node-to-node RPCs**: idle workers' nodes long-poll owners (`Fetch`), owners record dispatches (§10; built pull-based instead of offer/claim/grant streams, §19a) | One ownership domain, no SDK fan-out | You |
| D2 | Cluster size | **Unbounded by design**; the ceiling is `num_shards / 8` useful nodes | No shared hot object; per-node bucket traffic is O(1) | You |
| D3 | Failover target | **10–15 s** (lease timeout 10 s) | Few false takeovers, cheap | You |
| D4 | Verification | **TLA+ model + deterministic simulation + chaos** | Protocol bugs found before code | You |
| D5 | Task placement | **Queue partitions** (§9) | Cross-node traffic bounded at any size; FIFO per partition | You |
| D6 | Shard count | **Chosen at cluster creation**, a power of two in 16–65,536 (default 4096), stored in `cluster.json` | Large clusters possible without a migration | You |
| D7 | Deploy targets | **Kubernetes and VMs equally** | | You |
| D8 | Node security | **Cluster token always, mTLS optional (recommended)** | Safe by default, easy to deploy | Delegated to me |
| D9 | Shard ownership records | **One object per shard**, CAS'd only when the shard moves | No single hot object | Mine |
| D10 | Write fencing | **Each flush checks the node's own lease** (1 conditional GET) | O(1) per node at any cluster size; needs no clock | Mine |
| D11 | Snapshot and record ordering | **Per-shard ownership generation** (`gen`), bumped on every move | Orders snapshots across owners; per-node epochs cannot (see §19) | Mine |
| D12 | Takeover replay | **Log splitting**: one node splits a dead node's log into per-shard snapshots | Each dead log is read once, not once per claiming node | Mine |
| D13 | Membership | **The bucket for both** (revised from "gossip for suspicion": a LIST of `nodes/` is enough, §19a) | No second channel to secure; the bucket decides | Mine |
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
          │  Dispatcher: worker streams; remote feeder; result forwarding         │
          └───────┬──────────────────────────────────────────────┬───────────────┘
                  │ internal gRPC (token/mTLS)                   │
     ┌────────────┴──────────┐   Execute (commands, fetch,       ┌┴──────────────────────┐
     │ node A                │◄──dispatch, results), Scatter,───►│ node B                │
     │  shards it owns:      │   Events                          │  shards it owns:      │
     │   state in RAM        │                                   │   state in RAM        │
     │   WAL writer (own log)│   membership: LIST nodes/ (bucket)│   WAL writer (own log)│
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
| Shard count | const 4096 | `cluster.json.num_shards`, power of two, 16–65,536, default 4096 |
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
`{inc, state: alive|draining|dead, counter, internal_addr, api_addr, version, protocol, dead_by?, incarnations}`.
`incarnations` lists the current incarnation and every earlier one that is not split yet,
each with its **truncation watermark** `trunc`: the lowest position of that incarnation's
log that may still exist (§8.7). Only its owner writes it, except for exactly one kind of
write by another node: the tombstone (§7.4). Every write is a CAS (`If-Match`).

Once an incarnation is fenced (tombstoned, or superseded by a newer start) its owner can no
longer CAS the lease, so its watermark is **frozen**. Splits rely on that (§8.3).

Leases are **never deleted**. Deleting one restarts the node's incarnation counter, and a
reused incarnation id is still named by old split markers and snapshot pointers: the
split-log collector deletes the live reused incarnation's segments (the TLA+ model's
`LeaseGC` counterexample loses an acknowledged record this way), a slow splitter of the old
incarnation seals the new one's log, and when it dies claimers find "dead and split" and
skip its split. A paused process of the old incarnation would also pass its own-lease
checks again. A lease is a few hundred bytes per node id ever used.

### 7.2 Start

```
1. read or create cluster.json; refuse to start if num_shards / format differ from config
2. storage self-test (§12.1); refuse clustered mode on local/memory backends
3. read nodes/{me}/lease; CAS it to {inc: old.inc + 1, state: alive}        ← fences my previous run
   carrying every not-yet-split incarnation's watermark forward
4. if my previous incarnations owned shards: split their logs (§8.3)    ← as any dead node's
5. read the other leases (LIST nodes/): membership and peer addresses
6. wait join_delay, then take part in placement (§8.1); on a cold start (no node past its
   join delay) everyone takes part at once
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

1. **Suspicion.** Every `membership_interval` (3 s) each node LISTs `nodes/`, which returns
   every lease's ETag; a lease whose ETag has not changed for `lease_timeout` by this
   node's own monotonic clock is suspect. (Designed as gossip plus a backstop LIST; the
   LIST alone meets the failover target, §19a.)
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
1. read D's watermark w from nodes/{node}/lease          ← frozen: D is fenced (§7.1)
2. LIST wal/{node}/{inc}-* from w; the end is the first position ≥ w that is not listed,
   or an existing SEALED segment at or above w
3. PUT-create a SEALED empty segment at the end
   (if a late PUT just filled that position: LIST again, retry at the new end)
4. read the segments from w up to the seal (range GETs, in parallel); a segment that
   vanished meanwhile is skipped (only a finished concurrent split deletes there)
5. only then: if splits/{node}/{inc} exists, another split finished → stop, write nothing;
   else for each shard whose owner record names D (at gen g): load the shard's newest
   snapshot at gen g, apply D's records of (shard, g) above its shard_seq in log order,
   PUT-create a snapshot (g, last_seq, ptr = split) if there were any
6. PUT-create splits/{node}/{inc}                                         ← "split done"
7. delete D's segments (garbage collector backstop: §8.7)
```

Why it is safe:
- **Nothing acked is missed (G1).** A segment was acked only after its own-lease check
  (§12.3) passed. That check happened before the tombstone, which happened before this
  LIST, and the segment's PUT happened before the check. Acks resolve in order, so every
  position from w up to the highest acked one holds a segment, and none of them is deleted
  before D's split marker exists: truncation deletes only below a watermark it first CASed
  into the lease (§8.7), and a fenced incarnation can no longer CAS it. A LIST is
  linearizable per key, so a key that exists for the whole LIST is listed: the end found in
  step 2 lies above every acked segment.
- **Why not "the first gap after the lowest listed position".** The TLA+ model (§15.1)
  breaks both obvious alternatives. (a) A paused D wakes up after its tombstone and keeps
  truncating; a LIST racing those ascending deletes can see `{k, k+2, …}` with `k+1`
  already gone. "First gap" then seals `k+1` (create-if-absent succeeds on a deleted
  position) and every acked segment above it is lost once D's log is collected. (b) The
  lowest listed position can sit above a position whose PUT has not landed (segment 1 in
  flight, segment 2 landed). Reading from 2 skips shard_seqs: the split snapshot's history
  has a hole. `max(w, lowest listed)` still has (b). Starting at w and stopping at the first
  missing position has neither: holes below w are ignored, and a missing position at or
  above w is the end.
- **Nothing is added behind the seal.** A dead node's in-flight PUTs can land out of order.
  A segment after a gap was never acked, because acks resolve strictly in order. Sealing the
  gap makes the missing PUT fail, and every reader stops at the seal. Positions below the gap
  already exist and are create-if-absent, so they can't be overwritten.
- **A slow splitter re-checks the marker before writing.** Once a split has finished, step 7
  deletes the log *including the seal*, so a dead node's very late PUT can land at the
  sealed position again. A splitter that started before the marker and reads after it can
  then see a log nobody else saw: the model found one writing a split snapshot with a
  `shard_seq` hole that became the shard's newest snapshot. Re-checking the marker after
  reading closes it: a splitter that still finds no marker read before the marker existed,
  so before any GC, and the log it read is the one the finished split read.
- **Read, then load the owner records and bases.** Segments that vanish mid-read (only GC
  after a marker deletes at or above w) are skipped or fail the split. Either is safe only
  because what step 5 builds on is read *after* the log: by then it reflects any finished
  split (its snapshots cover what vanished) and any release. Loading them first is not:
  the model has a slow splitter load its base, the finished split's GC remove the log and,
  after the owner's release, the marker as well, so the re-check passes and the stale base
  plus the remains of the log make a snapshot with a hole.
- **Deterministic.** Two splitters that both write (both read before any marker) read the
  same log up to the same seal and write semantically identical snapshots; create-if-absent
  keeps the first.
- **Old generations are skipped.** If D owned a shard, released it and later re-claimed it,
  its log holds records of both generations. The older generation ended with a final
  snapshot, so step 5 rebuilds only the generation the owner record names. Grouping by every
  (shard, gen) in the log instead can apply an old generation's records to whatever
  snapshot of that generation pruning left behind, and write a malformed (if never loaded)
  snapshot; the model found that too.

A dead log is read **once**, by one node, instead of once per node that claims one of its
shards. The log budget (§12.5) bounds how much there is to read.

### 8.4 Claim (shard `S`, by the desired owner `B`)

```
1. GET owners/S → {owner, gen g, etag}
   allowed if: released, unowned, or owner's incarnation is dead AND splits/… exists
2. CAS owners/S → {B, inc_B, gen g+1, owned}                           ← S's history moves to B
3. load S's latest snapshot by (gen, seq), a split or final snapshot ranking above a node
   pointer at equal (gen, seq) (not plain key order: a node id can sort after "split");
   its ptr must be split, final, or point into a dead-and-split incarnation; a pointer
   into a dead but unsplit incarnation → split it first; anything else is a bug → refuse
   (no snapshot at all → start empty, at any gen: a claimer serves only once its
   acquisition snapshot exists and the newest snapshot is never pruned, so no snapshot
   means nobody ever served S, e.g. a claimer crashed between its CAS and step 4)
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
snapshot's `gen`, and its `shard_seq` is greater than the snapshot's. Read from the
incarnation's watermark and stop at a SEALED segment or at the first gap at or above it.
The `gen` filter is a second line of defence: a record from any other generation is
skipped and alerted, never applied. The applied records' `shard_seq`s must continue the
base's without a gap; a gap means a bug, never data to serve.

### 8.7 Truncation and garbage collection

Every delete is audited for *what if it runs arbitrarily late* (a zombie):

| Delete | Rule | Safe if run late because… |
|---|---|---|
| Own log truncation | first CAS the lease's `trunc` up to the bound (the minimum log pointer over my Serving shards' latest durable snapshots, and at most my acked position + 1); then delete below it, ascending. A failed CAS means fenced: poison | every snapshot a claimer could load points at or above that bound, and a split reads only from the frozen watermark up, so a late delete is always below where any split reads |
| Snapshot prune | keep the newest 2 per shard by (gen, seq); never the newest | the newest is never deleted |
| Dead-incarnation log | only after `splits/{node}/{inc}` exists | split snapshots cover everything in it |
| Split markers | after the incarnation's log is gone and no snapshot key and no owner record references it | nothing reads it; a claim needs the marker while an owner record names the incarnation |
| Leases | never | incarnation ids must never be reused (§7.1) |

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

Built pull-based (the offer/claim/grant streams of the first draft were not needed, §19a):

```
worker w7 ─ Session ─ node W                          node O (owns a partition shard of Q)
                      │ w7 idle on queue Q; O owns some of Q's partitions
                      │── Fetch{Q, wait 2 s} ───────► O: a runnable task T in Q's partitions?
                      │                                  reserve T for W (RAM only, lapses
                      │                                  after OFFER_TTL = 30 s)
                      ◄── T (spec, checkpoints) ─────
                      │ offer T to a waiting local worker; w7 takes it, run id minted by W
                      │── Dispatch{T, run, w7, node W} ► O records TaskDispatched{w7, node W}
                      ◄── assignment ────────────────
                      │ push to w7
```

- **Who fetches.** W runs one fetch loop per (queue, owner node) while some local worker
  waits for that queue, for the owners of that queue's partitions: at most `Σ P` loops over
  the queues W's workers serve, independent of cluster size.
- **Local first.** On O, a task that becomes runnable is offered to O's own waiting workers
  first (the sync-match hot path). Matching takes a task only when a worker is waiting, so a
  task nobody takes stays in O's pending index for local and remote takers alike.
- **No double hand-out.** A reservation is exclusive in O's RAM. A fetched task no local
  worker takes goes straight back (`Unoffer`); an unused reservation lapses after 30 s.
- **Dispatch is idempotent** on the run id W minted (a retried `Dispatch` returns the same
  run). A dispatch whose worker vanished before the assignment reached it is returned with
  a `RunReturned` record: back to PENDING **without spending an attempt**.

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
`NotOwner{owner}` redirects; after two redirects the owner record in the bucket decides. A
call that still finds no serving owner fails with `Unavailable` and the caller retries.

### 11.2 Internal RPCs

`proto/valka/v1/internal.proto`, served on `cluster.internal_addr`. These replaced the
PG-era `ForwardTask`, `ForwardEvent`, hash ring and partitions:
- `Execute(command)`: one JSON-encoded `valka_cluster::Command` addressed to a shard or to a
  node. Shard commands: create, get, cancel, delete, signal, runs, checkpoints, idempotency
  lookup, fetch, dispatch, return, unoffer, result, checkpoint, signal delivered/acked/reset,
  reattach, heartbeat batch. Node commands: cancel or deliver a signal to a local worker,
  drain, invalidate a queue's cached config.
- `Scatter(query)`: list, count, stats, workers, shard map, merged by the caller.
- `Events`: a server stream of this node's task events (the event relay).
- `Ping`: a reachability probe (`PeerClients::ping`, used by tests and tooling).

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

- `WorkerHello` gains `running: [{task_id, run_id, attempt}]` (all four SDKs); unacked
  results are re-sent on the new stream.
- W forwards each entry to the owner:
  - run known and RUNNING → record the worker's new node (`RunReattached`, asynchronous)
  - run unknown, task PENDING, `attempt = attempts + 1` → its dispatch record was lost in
    a crash: **adopt** it with a normal `TaskDispatched` carrying the worker's run id
  - task terminal or cancelled, or the run is stale → `Stale`; the worker cancels its copy
- After claiming shards, the owner holds re-dispatch of their recovered PENDING tasks for
  `reconcile_grace` (5 s) so reconnecting workers claim their runs first.

### 11.5 Events

- Every node relays every other live node's task events (`InternalService.Events`) into its
  own broadcast channel, so a subscriber on any node sees the whole cluster's events; filters
  apply on the subscriber's node.
- This fan-in is fine up to a few dozen nodes; beyond that, route per-task and per-queue
  subscriptions to the owners involved, or tail the WAL from the bucket (CDC).

### 11.6 Internal security (D8)

- A separate internal listener (`cluster.internal_addr`, default `:50052`). Never expose it
  publicly.
- Every internal call carries `cluster.token` (compared in constant time). Clustered mode
  refuses to start without a token.
- Optional mTLS (`cluster.tls_cert`, `cluster.tls_key`, `cluster.tls_ca`: PEM files,
  rustls). With mTLS the token is still required, as defence in depth. Recommended for
  production.

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

`spec/tla/Ownership.tla`, model-checked with TLC in CI (the `tla` job). Done in M1; see
`spec/tla/README.md` for what is abstracted and why that is sound.
- **State:** the bucket (leases with per-incarnation truncation watermarks, owner records,
  per-incarnation segments with positions, snapshots with (gen, seq, ptr), split markers);
  requests in flight (segment and snapshot PUTs, which land late, out of order, after their
  sender died); per incarnation: served shards and their phase, writer buffer,
  unacknowledged segments, acked position, pending snapshots; splitters.
- **Actions:** start, crash, tombstone (at any time), write, flush, PUT landing, own-lease
  check and ordered ack, snapshot capture / sync / PUT, claim (CAS, load, acquisition
  snapshot, serve), release (final snapshot, CAS), split (per-key LIST, seal, read, load
  bases, write, marker, abort), truncation (watermark CAS, ascending deletes), snapshot
  prune, dead-log deletion, marker GC. Pauses are interleavings.
- **Invariants:** `AckedDurable` (G1); `LinearHistory` (G2); `WellFormed` (no history has a
  gap in its `shard_seq`s); `NoLiveLogSealed`; `GCSafety` (from the bucket alone every
  acknowledged record is recoverable: nothing a future replay needs is deleted).
- **Liveness** under weak fairness: a shard whose owner is fenced is eventually served.
- **Size:** several small configurations instead of one large one (1–2 nodes, 1–2 shards,
  1–2 incarnations, up to 3 records, 1–2 concurrent splitters); the zombie case needs only
  one incarnation, because a tombstoned process may keep running. Protocol bugs show up at
  small sizes: every bug listed in §19 shows up within 45 steps.
- **Counterexamples** for each bug below stay in `spec/tla/counterexamples/`.

### 15.2 Deterministic multi-engine tests

2–5 engines on one shared in-memory store behind `FaultyStore`, with paused tokio time.
Most of M2–M7 is tested here: fast, reproducible, no network.

### 15.3 Simulation

`valka-engine/src/sim_tests.rs`, in-process instead of turmoil (gossip is gone, and node-to-
node RPCs do not affect ownership): 3–4 engines on one in-memory bucket, each behind its own
`FaultyStore`, with paused time. Randomized schedules of creates, results, kills, freezes and
thaws, drains, restarts, lost PUT responses and bucket errors; every acknowledged operation
goes into a history that is checked against G1 (every acked task is present with its acked
state), G2 (one owner per shard) and liveness (every shard served again). Seeds are
reproducible (`VALKA_SIM_SEED`, `VALKA_SIM_SEEDS`; 8 per test run, 400 checked before release). It found the
three claim bugs in §19a.

### 15.4 Chaos on real processes

`docker-compose.cluster.yml` (3 nodes and an S3-compatible store) driven by
`scripts/chaos.sh`: random `kill -9`, SIGSTOP freezes and SIGTERM restarts while
`examples/rs/chaos.rs` creates tasks through every node and runs workers on every node. It
checks that every acknowledged create exists and reaches a terminal state, and that no
idempotency key produced two tasks. Network splits and bucket slowdowns (toxiproxy) and the
24 h soak are not part of it yet.

Results so far (10-minute runs at 20 creates/s, ~9,500 acknowledged tasks each): no
acknowledged task lost and no duplicate key in any run. Three runs failed liveness or drain
expectations and led to the three fixes in §19b; the run after them passed with every task
completed (6 kills, 5 freezes, 7 SIGTERM restarts, every drain complete).

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
`nodes/` LIST per `membership_interval` (3 s) plus a GET per changed lease. Apart from that
LIST's size, nothing grows with cluster size. WAL segment PUTs come on top and
are unchanged from phase 1.

| Nodes | Coordination requests / s | ≈ S3 Standard cost / day | Notes |
|---|---|---|---|
| 10 | ~210 | ~$10 | dominated by per-flush checks at 20 flushes/s |
| 100 | ~2,100 | ~$100 | |
| 1,000 | ~21,000 | ~$1,000 | needs `num_shards ≥ 8,192`; the `nodes/` LIST is O(N) per node |

A takeover costs one CAS, a LIST and a range read of at most `log_budget` for the dead log,
plus per shard one GET and one or two snapshot PUTs.

Scale ceilings, stated honestly:
- Useful nodes ≈ `num_shards / 8`.
- A queue partition's throughput is bounded by one shard (one owner, one lock).
- The event relay (every node streams every other node's events) tops out around a few
  dozen nodes.

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
| Membership LIST cost at very large N | One LIST per node per interval; lengthen the interval, or add gossip as a suspicion accelerator |
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
- **What the TLA+ model found (M1).** Each has a counterexample in `spec/tla/counterexamples/`.
  - *Truncation racing a split loses acked records.* "Seal the first gap after the lowest
    listed position" is unsafe: a paused, tombstoned owner keeps truncating, and a LIST
    racing its deletes shows a deleted position as the end of the log. Fix: a truncation
    watermark in the lease, raised by CAS before any delete (frozen once fenced), and
    splits read from the watermark (§7.1, §8.3, §8.7).
  - *Starting at the lowest listed position skips records.* With segment 1 still in flight
    and segment 2 landed, the split applies `shard_seq` 2 without 1. Also true of
    `max(watermark, lowest listed)`. Fix: start exactly at the watermark; a missing position
    at or above it is the end (§8.3).
  - *Grouping by every (shard, gen) in the log* applies an old generation's records to an
    arbitrary surviving snapshot of that generation. Fix: rebuild only shards whose owner
    record names the dead incarnation, at that record's generation (§8.3 step 5).
  - *Concurrent splitters are not automatically deterministic.* GC deletes the seal with
    the log, a late PUT re-fills the sealed position, and a slow splitter reads a log the
    finished one never saw. Fix: re-check the split marker after reading, before writing,
    and load owner records and bases only after reading (§8.3 steps 4–5); with bases
    loaded first, a marker collected in the meantime defeats the re-check.
  - *"No snapshot ⇒ start empty, only at gen 0" blocks forever* after a claimer crashes
    between its CAS and its acquisition snapshot. "No snapshot ⇒ start empty" holds at any
    generation (§8.4 step 3).
  - *Deleting long-dead leases reuses incarnation ids*, and a stale split marker then lets
    the split-log collector delete the live reused incarnation's acked segments. Leases are
    never deleted (§7.1, §8.7).
  - Clarified, not bugs: snapshot order at equal (gen, seq) must rank split/final above a
    node pointer (§8.4); marker GC also waits for owner records (§8.7).

## 19a. Changes made while implementing M2–M5

The model's findings (§19) are all implemented, each with a regression test
(`valka-engine/src/safety_tests.rs`). Beyond them:

- **Membership is bucket-only (D13 revised).** One LIST of `nodes/` per
  `membership_interval` (3 s) plus a GET of a lease when its ETag changes; a lease whose
  ETag has not changed for `lease_timeout` by the observer's own clock is suspect, and its
  guardian tombstones it. Failover stays within 10–15 s without gossip, and there is no
  UDP port to secure. Cost is one LIST per node per interval plus GETs of changed leases,
  fine up to roughly a hundred nodes; gossip can come back as a suspicion accelerator if
  LIST cost matters at larger sizes.
- **Lease fields.** The lease's per-incarnation watermarks are stored as `trunc` (the
  current incarnation's) plus `unsplit: [{inc, trunc}]` (earlier incarnations of the node
  not split yet); `LeaseRecord::incarnations()` is the list §7.1 describes.
- **Per-shard log order.** Every record of a shard is appended inside the shard's critical
  section, so a shard's records appear in the log in `shard_seq` order. Coalesced
  heartbeat records once broke this (assigned under the lock, appended after it); replay
  would then have skipped them.
- **Three bugs found by the randomized multi-node simulation** (`valka-engine/src/sim_tests.rs`,
  hundreds of seeds of kills, freezes, drains and bucket faults with a history checker), all
  in claims, none in the protocol as modelled:
  - *Claims must list the shard's snapshots after their owner CAS.* Claims of a batch
    shared one LIST taken before the owner records were read; a split that finished in
    between (its marker is what made the shard claimable) wrote newer snapshots, and the
    claim loaded the stale one, losing acknowledged records.
  - *A claimer must finish a claim whose CAS may have landed.* Once the owner record names
    an incarnation, nobody else can claim the shard while it lives. A claim that failed
    after its CAS, or whose CAS response was lost, left the shard unserved forever when
    placement then preferred another node. Such shards are now "stranded" and retried by
    their claimer whatever placement says; once served, they are handed over normally.
  - *A release whose owner-record CAS has an unknown outcome must not serve again.* The
    CAS to "released" landed but its answer was lost; the owner took the error as "not
    released" and went back to serving while another node claimed the shard from the
    final snapshot, so writes the old owner acknowledged afterwards were lost (found later,
    in v0.2.0 work, by the same simulation). Now the shard is dropped and marked stranded:
    a claim settles it, at the next generation, which also fences a late-landing CAS.
    Regression test: `cluster_tests::a_release_whose_cas_answer_is_lost_never_leaves_two_writers`.
- **A claim that finds its newest snapshot pointing into a dead, unsplit log backs off**
  for the guardian's split rather than splitting inline: after the claim's own CAS the
  owner record names the claimer, so an inline split would skip the very shard.

## 19b. Changes made while implementing M6–M9

- **Cross-node matching is pull-based (§10).** Idle workers' nodes long-poll the owners of
  their queues' partitions; owners reserve in RAM and record the dispatch. No peer streams,
  no grants: the owner's pending index is the single place a runnable task waits, and a
  reservation that is never used simply lapses.
- **Matching accepts a task only for a waiting worker.** Buffering tasks in a node's matching
  layer would hide them from remote fetchers and strand them on a node that has no worker.
- **Heartbeats are batched per owner** each tick; cancels and signal deliveries go to the
  node recorded on the run (`TaskDispatched.node_id`, updated by `RunReattached`).
- **Reattach outcomes** are `Reattached` (record the new node), `Adopted` (the dispatch
  record was lost: record it now under the worker's run id) and `Stale` (send the worker a
  `TaskCancellation`). The SDKs' hello carries every running run.
- **Operations.** The admin API (drain, move/pin, unpin, partitions, audit) needs
  `VALKA_ADMIN_TOKEN` and writes an audit object per action; pins live in `placement.json`.
  The version gate refuses to start a node whose protocol is older than the effective
  protocol of the live cluster.
- **Verification.** The randomized simulation (§15.3) replaced the planned turmoil harness;
  the chaos rig (§15.4) runs three real processes in docker compose.
- **Found by the chaos rig: a drain must keep renewing its lease.** SIGTERM stopped every
  background loop, the lease keeper included, before handing shards over. Releasing ~1,400
  shards one at a time outlasted the lease timeout, a guardian tombstoned the draining node,
  and the rest of its shards went through a split instead of a handover (safe, but not the
  near-zero unavailability a drain promises). Lease renewal now stops only once the lease is
  marked dead, and releases run in parallel (`claim_concurrency`). Regression test:
  `cluster_tests::a_drain_slower_than_the_lease_timeout_keeps_its_lease`.
- **Also seen in the chaos logs: a reattach that hits a moving shard is retried.** A worker
  that reconnects while its task's shard is changing owner got `Unavailable` for its
  reattach, and the dispatcher gave up: the owner kept the run's old node (cancels and
  signals went there), and a lost dispatch was not adopted, so the task would run twice
  once the claim's grace ended. The dispatcher now retries in the background (backoff up to
  5 s, for 60 s) while the worker still runs the task.
- **Found by the chaos rig: an offer whose dispatch fails must go back.** A task the owner's
  own feeder handed to local matching is marked offered and leaves the pending index. If the
  worker's dispatch then failed (here: the owner briefly not serving after a slow lease
  renewal), the dispatcher dropped it, assuming a reservation would lapse; only remote
  fetches had one. Three acknowledged tasks stayed PENDING forever (none lost, none run).
  Now a refused dispatch hands the task back at once (`unoffer`), and every offer, local or
  remote, lapses after `OFFER_TTL` (30 s) as a safety net. Regression tests:
  `a_local_offer_that_is_never_dispatched_lapses_back_to_pending`,
  `test_a_failed_dispatch_hands_the_task_back_at_once`.
- **Found by the chaos rig: publishing after durability must not depend on the caller.**
  A create forwarded by a node that was then killed had its request cancelled while it
  waited for its group commit. The record became durable, but the code after the wait
  (publish events, index and offer the task, arm timers) ran in the cancelled future, so
  the task was durable yet never runnable until its shard moved. Every command now waits
  and publishes in its own task (`Engine::commit`) and the caller only awaits the outcome.
  Regression test: `a_create_whose_caller_goes_away_is_still_published_once_durable`.

## 20. Open questions

- Automatic partition scaling and load-aware placement: phase 3.
- Read-your-writes across a claim for list endpoints: reads served from RAM may include
  writes still in flight. The same as phase 1, documented; a "durable reads" option is
  possible later.
- Cloudflare R2's conditional-write and LIST semantics: confirm with the self-test before
  listing it as supported.
- `ack=fast` and payload blobs: independent of this plan; they fit after M5.
- Helm chart and systemd units (M10), network-split and bucket-slowdown chaos, the 24 h soak
  (M9): next.
