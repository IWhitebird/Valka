# TLA+ model of phase-2 ownership

`Ownership.tla` models the protocol in [`docs/wal/PHASE2.md`](../../docs/wal/PHASE2.md)
§7–§8 and §12: node leases and incarnations, tombstones, per-shard owner records with
generations, the writer's own-lease check before releasing acks, snapshots, claims with an
acquisition snapshot, releases with a final snapshot, sealing, log splitting, own-log
truncation with a watermark, snapshot pruning, dead-log deletion and split-marker GC. TLC
checks it in CI (the `tla` job).

## What is modelled

**Bucket.** `lease` (incarnation, alive/dead), `wm` (each incarnation's truncation
watermark, which lives in the lease), `owner` (per shard: node, incarnation, generation,
owned/released), `log` (per incarnation and position: absent, SEALED, or a segment of
records), `snaps` (per shard: snapshots keyed by generation, history and log pointer),
`marker` (split markers).

**Requests in flight.** Segment and snapshot PUTs sit in `segPuts` / `snapPuts` until they
land, which can be at any later time, out of order, and after their sender died. A segment
PUT is create-if-absent.

**Processes.** One per (node, incarnation). Each has the shards it serves (with their
history and phase: loading, acquisition pending, serving, final pending), a writer buffer,
the segments sent but not acknowledged, the highest acknowledged position, snapshots
waiting for their sync, and the newest own snapshot per shard known to be durable.

**Shard state is its history**: the sequence of record ids applied, so the record with
`shard_seq = j` sits at index `j`. Records are created by `Write` on a served shard, never
validated against the lease (a zombie keeps writing).

**Splitters** run for any fenced incarnation, possibly several at once (`Splitters`):
LIST one key at a time, seal, read, load bases, write split snapshots, write the marker.
They may abort at any point and start over.

| PHASE2 step | Actions |
|---|---|
| §7.2 start (lease CAS, inc + 1) | `Start` |
| §7.4 tombstone | `Tombstone` (at any time: safety must not depend on clocks) |
| crash, kill -9, giving up | `Crash` (RAM lost, PUTs keep flying); `SelfFence` (§7.3: a fenced process exits) |
| §5 write path, group commit | `Write`, `Flush`, `LandSeg` |
| §12.2–12.3 PUT read-back + own-lease check, ordered acks | `Commit` |
| §12.4 snapshot, sync, PUT | `SnapCapture`, `SnapSync`, `SnapLand` |
| §8.4 claim | `ClaimCAS`, `ClaimLoad`, `ClaimServe` |
| §8.5 release | `ReleaseFinal`, `ReleaseCAS` |
| §8.3 split | `SplitBegin`, `SplitList`, `SplitSeal`, `SplitRead`, `SplitWrite`, `SplitMark`, `SplitAbort` |
| §8.7 truncation | `TruncMark` (lease CAS of the watermark), `TruncDel` (ascending deletes) |
| §8.7 GC | `PruneSnap`, `DeadLogDelete`, `MarkerGC`, `LeaseGC` (only with `GcLeases`) |

## What is abstracted, and why that is sound

- **Pauses** are not an action. In an interleaving model any process can stop taking steps
  for arbitrarily long and then continue from its stale state, which is exactly a pause.
- **Time, renewals and failure detection.** Safety may not depend on clocks, so the guardian
  may tombstone at any moment, including while the node is alive and renewing; a renewal
  changes nothing else, so it is not modelled. The tombstone's CAS on an observed ETag
  either finds the lease unchanged (equivalent to an atomic tombstone now) or fails.
  The liveness model (`LiveModel`) restricts tombstones to crashed incarnations.
- **Placement and membership.** Any process may claim any claimable shard and release any
  shard it serves, at any time. Rendezvous placement, gossip and rate limits only choose
  among these behaviours.
- **CAS.** Every CAS is one atomic step that checks the condition and writes.
- **LIST** reads one key per step, so concurrent deletes and landings show through in any
  mix (per-key linearizable, not an atomic prefix snapshot).
- **Reading segments** is one atomic step. Segments are immutable and, in the range a
  splitter reads, can only disappear, so any order of individual GETs gives the same
  result as an atomic read at some point.
- **Records** carry no payload; the task state machine (`apply`) is total and deterministic,
  so a history of record ids determines the state.
- **Timers, matching, workers, reads and task logs** are out of scope: they do not write the
  bucket except through the records above.
- **Bounds.** `MaxRec` records in total, `MaxInc` incarnations per node, `MaxGen` ownership
  moves per shard, `MaxRec + 1` positions per log.

## Invariants

| Name | Guarantee | Plain words |
|---|---|---|
| `AckedDurable` | G1 | A process that loads a shard at generation g holds every acknowledged record of every older generation, at its `shard_seq`. |
| `LinearHistory` | G2 | No two acknowledged records share a `(shard, shard_seq)`; every stored or loaded history agrees with every acknowledged record it reaches; snapshots of one generation are prefixes of one another. |
| `WellFormed` | — | Every stored or loaded history is gap-free: the record at index j has `shard_seq` j. |
| `NoLiveLogSealed` | — | A live incarnation's log never holds a SEALED segment. |
| `GCSafety` | G1 in the bucket | From the bucket alone every acknowledged record is recoverable the way a split recovers it: it is in each newest snapshot of its shard, or that snapshot was written by the record's incarnation at its generation and the record's segment is reachable from that log's watermark without a gap. |
| `EventuallyServed` | G5 | A shard whose owner is fenced is eventually served by a live incarnation (`Liveness.cfg`). |

## Configurations

| File | Checks | Size |
|---|---|---|
| `Ownership.cfg` | safety | 1 node, 1 shard, 1 incarnation, 3 records: the zombie case (fenced writer keeps flushing, snapshotting and truncating while splitters run) |
| `Restart.cfg` | safety | 1 node, 1 shard, 2 incarnations, 2 records |
| `TwoNodes.cfg` | safety | 2 nodes, 2 shards, 1 incarnation each, 2 records, symmetry |
| `Splitters.cfg` | safety | 1 node, 1 shard, 2 concurrent splitters, 2 records |
| `Liveness.cfg` | liveness | 2 nodes (one never fails), 1 shard, 2 incarnations, 1 record |

Results of the last full run (TLC 1.7.4, 2 workers, capped at 2 cores of a laptop):

| Configuration | Mode | Result | States | Time |
|---|---|---|---|---|
| `Ownership.cfg` | exhaustive | all invariants hold | 7,163,671 distinct (63.7 M generated) | 6 min |
| `Splitters.cfg` | exhaustive | all invariants hold | 9,184,292 distinct (88.8 M generated) | 7 min |
| `Liveness.cfg` | exhaustive | invariants and `EventuallyServed` hold | 99,756 distinct | 23 s |
| `Restart.cfg` | simulation, 200,000 behaviours, depth 80 | no violation | 11.7 M checked | 2 min |
| `TwoNodes.cfg` | simulation, 200,000 behaviours, depth 80 | no violation | 14.8 M checked | 3 min |
| `counterexamples/*.cfg` | exhaustive | each violates its property, as it must | — | 1 s – 8 min each |

`Restart.cfg` and `TwoNodes.cfg` did not finish exhaustively within 90 minutes on 2 cores; CI
simulates them on every push and checks them exhaustively in a nightly job
(`.github/workflows/tla-nightly.yml`).

`counterexamples/` holds configurations that **must fail**; each reproduces a protocol bug
the model found in an earlier draft (see below). CI checks that they still fail, so a change
to the model cannot quietly stop exercising a known failure.

## What the model found

Each of these was a TLC counterexample against the protocol as first written; the fix is
in PHASE2.md (§19 lists them) and in the model. The `counterexamples/` configuration flips
the relevant constant back.

| # | Bug | Counterexample | Fix |
|---|---|---|---|
| 1 | Split starts at the lowest listed position. A tombstoned but still running owner keeps truncating ascending; a LIST racing it sees `{1, 3}` with 2 just deleted, the splitter seals 2 and every acked segment above it is lost when the split log is collected. | `DraftSplit.cfg` (GCSafety, 44 steps) | Truncation watermark in the lease, raised by CAS before deleting (frozen once fenced); splits start at it (§7.1, §8.3, §8.7). |
| 2 | Split starts at `max(watermark, lowest listed)`. Segment 1 still in flight, segment 2 landed: the split applies `shard_seq` 2 without 1. | `MaxLow.cfg` (WellFormed, 23 steps) | Start exactly at the watermark; a missing position at or above it is the end (§8.3). |
| 3 | Split groups records by every (shard, gen) in the log. An old generation's records get applied to whatever snapshot of that generation pruning left (here: its acquisition snapshot), giving a malformed snapshot. | (fixed in the model; found at 37 steps) | Rebuild only shards whose owner record names the dead incarnation, at that record's generation (§8.3 step 5). |
| 4 | Splitters don't re-check the marker. GC deletes the split log including the seal, a late PUT re-fills the sealed position, a slow splitter reads it and writes a split snapshot with a `shard_seq` hole that becomes the newest. | `NoMarkerRecheck.cfg` (WellFormed, 29 steps) | Re-check `splits/{node}/{inc}` after reading, before writing (§8.3 step 5). |
| 5 | A splitter loads owner records and bases before reading. The finished split's GC removes the log and, once a release left no owner record naming the incarnation, the marker too; the slow splitter's re-check passes and it applies the rest of the log to its stale base. | `BasesFirst.cfg` (WellFormed, 40 steps) | Read the log first, then load owner records and bases (§8.3 steps 4–5). |
| 6 | "No snapshot ⇒ start empty" only at gen 1. A claimer that crashes between its CAS and its acquisition snapshot leaves gen 1 and no snapshot; every later claim refuses forever. | `StrictEmptyClaim.cfg` (EventuallyServed) | No snapshot ⇒ start empty at any gen (§8.4 step 3). |
| 7 | GC deletes long-dead leases. The node's incarnation counter restarts; the old split marker for incarnation 1 is still there, so the split-log collector deletes the live, reused incarnation 1's acked segment. | `LeaseGC.cfg` (GCSafety, 31 steps) | Leases are never deleted (§7.1, §8.7). |

Two more things the model settled. GCSafety first demanded an unbroken log from a
snapshot's *pointer*, and failed in `TwoNodes.cfg`: an acquisition snapshot's pointer can
lie below the watermark once other shards' snapshots let truncation pass it. That was the
invariant being wrong, not the protocol: a split reads from the watermark, never from a
pointer. And snapshot order at equal (gen, seq) must rank split/final above a node
pointer, which the model assumes.

## Running

Java 11+ and TLC (the CI pins v1.7.4):

```sh
cd spec/tla
curl -sLo tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
./check.sh                                    # every configuration above
./check.sh Restart.cfg                        # one of them
./check.sh --expect-fail counterexamples/*.cfg
```

`check.sh` runs `tlc2.TLC -deadlock -workers auto`. `-deadlock` turns off deadlock
detection: with bounded records and incarnations every behaviour ends once everything is
spent, which is not a deadlock of the protocol. To read a counterexample trace, run TLC
directly: `java -cp tla2tools.jar tlc2.TLC -deadlock -config counterexamples/DraftSplit.cfg
Ownership.tla`.
