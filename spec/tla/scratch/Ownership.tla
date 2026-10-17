------------------------------ MODULE Ownership ------------------------------
(***************************************************************************)
(* Valka phase 2: shard ownership, fencing, sealing, log splitting and     *)
(* garbage collection on a bucket. The protocol is docs/wal/PHASE2.md; the *)
(* section numbers below refer to it. spec/tla/README.md says what is      *)
(* abstracted and why that is sound.                                       *)
(*                                                                         *)
(* A shard's state is modelled as its history: the sequence of record ids  *)
(* applied to it, so record r sits at index r.seq (its shard_seq).         *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Node,             \* node ids (a symmetry set)
    Shard,            \* shard ids (a symmetry set)
    NoNode,           \* model value standing for "no node"
    MaxInc,           \* incarnations per node
    MaxRec,           \* client records appended over a whole behaviour
    MaxGen,           \* ownership moves per shard
    Splitters,        \* concurrent splitters per dead incarnation, e.g. {1} or {1, 2}
    SplitRule,        \* "draft" | "maxlow" | "watermark": where a split starts reading
    StrictEmptyClaim, \* TRUE: a claim refuses "no snapshot at all" above gen 1 (draft §8.4)
    LiveModel         \* TRUE: tombstone only crashed incarnations (liveness runs)

ASSUME /\ MaxInc \in Nat \ {0}
       /\ MaxRec \in Nat
       /\ MaxGen \in Nat \ {0}
       /\ SplitRule \in {"draft", "maxlow", "watermark"}
       /\ StrictEmptyClaim \in BOOLEAN
       /\ LiveModel \in BOOLEAN

Inc    == 1..MaxInc
Proc   == Node \X Inc            \* one process per (node, incarnation)
MaxPos == MaxRec + 2             \* positions in one incarnation's log
Pos    == 1..MaxPos

Range(f) == {f[j] : j \in DOMAIN f}
Min(S)   == CHOOSE m \in S : \A y \in S : m <= y
Max(a, b) == IF a >= b THEN a ELSE b

\* ---------------------------------------------------------------- values --

\* A log position holds nothing, a SEALED segment, or a segment of records.
Absent  == [k |-> "absent", c |-> <<>>]
Sealed  == [k |-> "sealed", c |-> <<>>]
Data(c) == [k |-> "data", c |-> c]

\* Snapshots: generation, history, and the log pointer that is part of the key.
\* k = "node": replay <<n, i>>'s log from position q; "split" / "final": nothing to replay.
Snap(g, h, k, n, i, q) == [gen |-> g, hist |-> h, k |-> k, n |-> n, i |-> i, q |-> q]
NodeSnap(g, h, p, q)   == Snap(g, h, "node", p[1], p[2], q)
SplitSnap(g, h)        == Snap(g, h, "split", NoNode, 0, 0)
FinalSnap(g, h)        == Snap(g, h, "final", NoNode, 0, 0)
NoSnap                 == Snap(0, <<>>, "none", NoNode, 0, 0)

\* Snapshot order: (gen, shard_seq), then split/final above a node pointer.
Rank(x) == IF x.k = "node" THEN 0 ELSE 1
Greater(y, x) ==
    \/ y.gen > x.gen
    \/ y.gen = x.gen /\ Len(y.hist) > Len(x.hist)
    \/ y.gen = x.gen /\ Len(y.hist) = Len(x.hist) /\ Rank(y) > Rank(x)

NoSrv     == [st |-> "none", gen |-> 0, hist |-> <<>>, x |-> NoSnap]
NoDur     == [seq |-> 0, q |-> 0]
IdleSplit == [st |-> "idle", cur |-> 0, seen |-> [q \in Pos |-> "absent"],
              start |-> 0, end |-> 0, got |-> <<>>, xs |-> {}]

\* ------------------------------------------------------------- variables --

VARIABLES
    \* bucket
    lease,      \* node -> [inc, st]: nodes/{node}/lease (§7.1)
    wm,         \* incarnation -> truncation watermark, stored in the lease (§8.7)
    owner,      \* shard -> [n, i, gen, st]: owners/{hh}/{shard} (§8.2)
    log,        \* incarnation -> position -> content: wal/{node}/{inc}-{seq}.seg
    snaps,      \* shard -> set of snapshots: snapshots/{hh}/{shard}/{gen}-{seq}-{ptr}
    marker,     \* set of incarnations: splits/{node}/{inc}
    \* requests in flight: they may land at any time, also after their sender died
    segPuts,    \* create-if-absent segment PUTs
    snapPuts,   \* snapshot PUTs
    \* processes, one per incarnation
    pst,        \* "unborn" | "running" | "gone"
    srv,        \* incarnation -> shard -> what it serves
    buf,        \* writer buffer: records appended, not yet in a segment
    wpos,       \* next segment position
    pend,       \* segments PUT but not yet acknowledged, in order
    ackedPos,   \* highest position whose acks were released
    snapQ,      \* snapshots captured, waiting for their sync
    durSnap,    \* shard -> [seq, q] of the newest own snapshot known to be durable
    \* splitters
    spl,
    \* history (not part of the system)
    rec,        \* every record ever appended: [shard, gen, seq, n, i]
    acked       \* ids of records acknowledged to a client

bucketVars  == <<lease, wm, owner, log, snaps, marker>>
flightVars  == <<segPuts, snapPuts>>
procVars    == <<pst, srv, buf, wpos, pend, ackedPos, snapQ, durSnap>>
ghostVars   == <<rec, acked>>
vars        == <<bucketVars, flightVars, procVars, spl, ghostVars>>

\* --------------------------------------------------------------- helpers --

Running(p)   == pst[p] = "running"
\* The incarnation is fenced: superseded by a restart or tombstoned.
IncDead(d)   == \/ lease[d[1]].inc > d[2]
                \/ lease[d[1]].inc = d[2] /\ lease[d[1]].st = "dead"
LiveLease(p) == lease[p[1]].inc = p[2] /\ lease[p[1]].st = "alive"
IsSplit(d)   == IncDead(d) /\ d \in marker
OwnerIs(s, d) == owner[s].st = "owned" /\ owner[s].n = d[1] /\ owner[s].i = d[2]
RefersTo(x, d) == x.k = "node" /\ x.n = d[1] /\ x.i = d[2]

Maximal(s)     == {x \in snaps[s] : \A y \in snaps[s] : ~Greater(y, x)}
MaximalAt(s, g) == {x \in snaps[s] : x.gen = g /\
                       \A y \in snaps[s] : y.gen = g => ~Greater(y, x)}

\* §8.4 step 1: who may claim.
ClaimOK(s) == \/ owner[s].st \in {"none", "released"}
              \/ owner[s].st = "owned" /\ IsSplit(<<owner[s].n, owner[s].i>>)

\* §8.4 step 3: a snapshot a claimer may serve from without replaying anything.
Acceptable(x) == x.k \in {"split", "final"} \/ IsSplit(<<x.n, x.i>>)

\* Records of shard s still in the writer (buffer or unacknowledged segments).
Unacked(p, s) ==
    \/ \E j \in DOMAIN buf[p] : rec[buf[p][j]].shard = s
    \/ \E j \in DOMAIN pend[p] : \E m \in DOMAIN pend[p][j].c : rec[pend[p][j].c[m]].shard = s

\* §8.7: own log truncation bound.
Bound(p) == Min({durSnap[p][s].q : s \in {t \in Shard : srv[p][t].st \in {"serving", "relwait"}}}
                \cup {ackedPos[p] + 1})

\* An incarnation that a split has to deal with.
NeedsSplit(d) == \/ \E q \in Pos : log[d][q].k # "absent"
                 \/ \E s \in Shard : OwnerIs(s, d)
                 \/ \E s \in Shard : \E x \in snaps[s] : RefersTo(x, d)

\* The process loses its whole RAM state; requests already sent stay in flight.
Poison(p) ==
    /\ pst'      = [pst EXCEPT ![p] = "gone"]
    /\ srv'      = [srv EXCEPT ![p] = [s \in Shard |-> NoSrv]]
    /\ buf'      = [buf EXCEPT ![p] = <<>>]
    /\ wpos'     = [wpos EXCEPT ![p] = 1]
    /\ pend'     = [pend EXCEPT ![p] = <<>>]
    /\ ackedPos' = [ackedPos EXCEPT ![p] = 0]
    /\ snapQ'    = [snapQ EXCEPT ![p] = {}]
    /\ durSnap'  = [durSnap EXCEPT ![p] = [s \in Shard |-> NoDur]]
    /\ UNCHANGED <<bucketVars, flightVars, spl, ghostVars>>

\* ------------------------------------------------------------------ init --

Init ==
    /\ lease    = [n \in Node |-> [inc |-> 0, st |-> "none"]]
    /\ wm       = [p \in Proc |-> 1]
    /\ owner    = [s \in Shard |-> [n |-> NoNode, i |-> 0, gen |-> 0, st |-> "none"]]
    /\ log      = [p \in Proc |-> [q \in Pos |-> Absent]]
    /\ snaps    = [s \in Shard |-> {}]
    /\ marker   = {}
    /\ segPuts  = {}
    /\ snapPuts = {}
    /\ pst      = [p \in Proc |-> "unborn"]
    /\ srv      = [p \in Proc |-> [s \in Shard |-> NoSrv]]
    /\ buf      = [p \in Proc |-> <<>>]
    /\ wpos     = [p \in Proc |-> 1]
    /\ pend     = [p \in Proc |-> <<>>]
    /\ ackedPos = [p \in Proc |-> 0]
    /\ snapQ    = [p \in Proc |-> {}]
    /\ durSnap  = [p \in Proc |-> [s \in Shard |-> NoDur]]
    /\ spl      = [d \in Proc |-> [k \in Splitters |-> IdleSplit]]
    /\ rec      = <<>>
    /\ acked    = {}

\* ------------------------------------------------------- node lifecycle --

\* §7.2 step 3: CAS the lease to the next incarnation. This fences the previous
\* one, which may still be running (paused) and becomes a zombie. Its watermark
\* is frozen from here on: in the bucket it is carried forward in the lease.
Start(n) ==
    LET i == lease[n].inc + 1 IN
    /\ lease[n].inc < MaxInc
    /\ lease' = [lease EXCEPT ![n] = [inc |-> i, st |-> "alive"]]
    /\ pst'   = [pst EXCEPT ![<<n, i>>] = "running"]
    /\ UNCHANGED <<wm, owner, log, snaps, marker, flightVars, srv, buf, wpos, pend,
                   ackedPos, snapQ, durSnap, spl, ghostVars>>

\* kill -9, machine loss, or giving up after failed PUTs.
Crash(p) == Running(p) /\ Poison(p)

\* §7.4: the guardian's CAS on the lease. Safety must not depend on clocks, so it
\* may happen at any time; a process that is merely paused gets tombstoned too.
Tombstone(n) ==
    /\ lease[n].st = "alive"
    /\ LiveModel => pst[<<n, lease[n].inc>>] = "gone"
    /\ lease' = [lease EXCEPT ![n].st = "dead"]
    /\ UNCHANGED <<wm, owner, log, snaps, marker, flightVars, procVars, spl, ghostVars>>

\* ---------------------------------------------------------- write path --

\* A command on a served shard: apply to RAM, append to the writer. No lease check.
Append(p, s) ==
    /\ Running(p)
    /\ srv[p][s].st = "serving"
    /\ Len(rec) < MaxRec
    /\ LET id == Len(rec) + 1
           r  == [shard |-> s, gen |-> srv[p][s].gen, seq |-> Len(srv[p][s].hist) + 1,
                  n |-> p[1], i |-> p[2]]
       IN /\ rec' = Append(rec, r)
          /\ srv' = [srv EXCEPT ![p][s].hist = Append(@, id)]
          /\ buf' = [buf EXCEPT ![p] = Append(@, id)]
    /\ UNCHANGED <<bucketVars, flightVars, pst, wpos, pend, ackedPos, snapQ, durSnap, spl, acked>>

\* Group commit: the buffer becomes a segment, PUT create-if-absent.
Flush(p) ==
    /\ Running(p)
    /\ buf[p] # <<>>
    /\ wpos[p] <= MaxPos
    /\ segPuts' = segPuts \cup {[n |-> p[1], i |-> p[2], q |-> wpos[p], c |-> buf[p]]}
    /\ pend'    = [pend EXCEPT ![p] = Append(@, [q |-> wpos[p], c |-> buf[p]])]
    /\ wpos'    = [wpos EXCEPT ![p] = @ + 1]
    /\ buf'     = [buf EXCEPT ![p] = <<>>]
    /\ UNCHANGED <<bucketVars, snapPuts, pst, srv, ackedPos, snapQ, durSnap, spl, ghostVars>>

\* A segment PUT lands (possibly long after its writer died). Create-if-absent:
\* a sealed or otherwise taken position makes it fail.
LandSeg(u) ==
    /\ segPuts' = segPuts \ {u}
    /\ log' = IF log[<<u.n, u.i>>][u.q].k = "absent"
              THEN [log EXCEPT ![<<u.n, u.i>>][u.q] = Data(u.c)]
              ELSE log
    /\ UNCHANGED <<lease, wm, owner, snaps, marker, snapPuts, procVars, spl, ghostVars>>

\* §12.2 + §12.3: the oldest unacknowledged segment is resolved. If the stored bytes
\* are ours and the own-lease check passes, its acks are released; anything else
\* (sealed position, fenced lease) poisons the process. Acks release strictly in order.
Commit(p) ==
    /\ Running(p)
    /\ pend[p] # <<>>
    /\ LET h == Head(pend[p]) IN
       /\ log[p][h.q].k # "absent"
       /\ IF log[p][h.q] = Data(h.c) /\ LiveLease(p)
          THEN /\ acked'    = acked \cup Range(h.c)
               /\ ackedPos' = [ackedPos EXCEPT ![p] = h.q]
               /\ pend'     = [pend EXCEPT ![p] = Tail(@)]
               /\ UNCHANGED <<bucketVars, flightVars, pst, srv, buf, wpos, snapQ, durSnap,
                              spl, rec>>
          ELSE Poison(p)

\* ------------------------------------------------------------- snapshots --

\* §12.4: capture a dirty shard. The pointer is the writer's next position; the
\* capture must wait for every segment written so far (the sync).
SnapCapture(p, s) ==
    /\ Running(p)
    /\ srv[p][s].st = "serving"
    /\ buf[p] = <<>>
    /\ Len(srv[p][s].hist) > durSnap[p][s].seq
    /\ ~\E e \in snapQ[p] : e.s = s
    /\ snapQ' = [snapQ EXCEPT ![p] = @ \cup
                   {[s |-> s,
                     x |-> NodeSnap(srv[p][s].gen, srv[p][s].hist, p, wpos[p]),
                     need |-> IF pend[p] = <<>> THEN 0 ELSE pend[p][Len(pend[p])].q]}]
    /\ UNCHANGED <<bucketVars, flightVars, pst, srv, buf, wpos, pend, ackedPos, durSnap,
                   spl, ghostVars>>

\* The sync: everything captured is acknowledged, and the sync's own segment passes the
\* own-lease check. Only then is the snapshot PUT. A failed sync poisons.
SnapSync(p, e) ==
    /\ Running(p)
    /\ ackedPos[p] >= e.need
    /\ IF LiveLease(p)
       THEN /\ snapPuts' = snapPuts \cup {[s |-> e.s, x |-> e.x]}
            /\ snapQ'    = [snapQ EXCEPT ![p] = @ \ {e}]
            /\ UNCHANGED <<bucketVars, segPuts, pst, srv, buf, wpos, pend, ackedPos,
                           durSnap, spl, ghostVars>>
       ELSE Poison(p)

\* A snapshot PUT lands. Keys are unique, so create-if-absent is a set union. Its writer
\* learns it is durable only if it is still around to see the response.
SnapLand(u) ==
    /\ snapPuts' = snapPuts \ {u}
    /\ snaps'    = [snaps EXCEPT ![u.s] = @ \cup {u.x}]
    /\ LET p == <<u.x.n, u.x.i>> IN
       durSnap' = IF /\ u.x.k = "node"
                     /\ p \in Proc
                     /\ Running(p)
                     /\ srv[p][u.s].st = "serving"
                     /\ srv[p][u.s].gen = u.x.gen
                     /\ Len(u.x.hist) > durSnap[p][u.s].seq
                  THEN [durSnap EXCEPT ![p][u.s] = [seq |-> Len(u.x.hist), q |-> u.x.q]]
                  ELSE durSnap
    /\ UNCHANGED <<lease, wm, owner, log, marker, segPuts, pst, srv, buf, wpos, pend,
                   ackedPos, snapQ, spl, ghostVars>>

\* §8.7: keep the newest two snapshots of a shard; never the newest.
PruneSnap(s, x) ==
    /\ Cardinality({y \in snaps[s] : Greater(y, x)}) >= 2
    /\ snaps' = [snaps EXCEPT ![s] = @ \ {x}]
    /\ UNCHANGED <<lease, wm, owner, log, marker, flightVars, procVars, spl, ghostVars>>

\* ------------------------------------------------------- own truncation --

\* §8.7: raise the truncation watermark to the bound. With a watermark (maxlow,
\* watermark) it is a CAS on the own lease, so a fenced incarnation cannot move it and
\* is poisoned instead. The draft has no watermark: the bound is just a local number.
TruncMark(p) ==
    /\ Running(p)
    /\ Bound(p) > wm[p]
    /\ IF SplitRule = "draft" \/ LiveLease(p)
       THEN /\ wm' = [wm EXCEPT ![p] = Bound(p)]
            /\ UNCHANGED <<lease, owner, log, snaps, marker, flightVars, procVars, spl,
                           ghostVars>>
       ELSE Poison(p)

\* Delete segments below the watermark, one at a time, ascending, as the writer does.
\* No lease check: a zombie keeps deleting.
TruncDel(p) ==
    /\ Running(p)
    /\ \E q \in Pos : q < wm[p] /\ log[p][q].k = "data"
    /\ LET q == Min({q2 \in Pos : q2 < wm[p] /\ log[p][q2].k = "data"}) IN
       log' = [log EXCEPT ![p][q] = Absent]
    /\ UNCHANGED <<lease, wm, owner, snaps, marker, flightVars, procVars, spl, ghostVars>>

\* --------------------------------------------------------------- claims --

\* §8.4 step 2: CAS the owner record to (me, gen + 1).
ClaimCAS(p, s) ==
    /\ Running(p)
    /\ srv[p][s].st = "none"
    /\ ClaimOK(s)
    /\ owner[s].gen < MaxGen
    /\ owner' = [owner EXCEPT ![s] = [n |-> p[1], i |-> p[2], gen |-> owner[s].gen + 1,
                                      st |-> "owned"]]
    /\ srv' = [srv EXCEPT ![p][s] = [st |-> "loading", gen |-> owner[s].gen + 1,
                                     hist |-> <<>>, x |-> NoSnap]]
    /\ UNCHANGED <<lease, wm, log, snaps, marker, flightVars, pst, buf, wpos, pend,
                   ackedPos, snapQ, durSnap, spl, ghostVars>>

\* §8.4 steps 3-4: load the newest snapshot (any of the maximal ones), check it may be
\* served from, and PUT the acquisition snapshot pointing at my next position.
\* No snapshot at all: start empty (StrictEmptyClaim: only at gen 1).
ClaimLoad(p, s) ==
    /\ Running(p)
    /\ srv[p][s].st = "loading"
    /\ \E base \in IF snaps[s] = {} THEN {NoSnap} ELSE Maximal(s) :
          /\ IF base = NoSnap
             THEN ~StrictEmptyClaim \/ srv[p][s].gen = 1
             ELSE Acceptable(base)
          /\ LET acq == NodeSnap(srv[p][s].gen, base.hist, p, wpos[p]) IN
             /\ snapPuts' = snapPuts \cup {[s |-> s, x |-> acq]}
             /\ srv' = [srv EXCEPT ![p][s] = [st |-> "acqwait", gen |-> @.gen,
                                              hist |-> base.hist, x |-> acq]]
    /\ UNCHANGED <<bucketVars, segPuts, pst, buf, wpos, pend, ackedPos, snapQ, durSnap,
                   spl, ghostVars>>

\* §8.4 step 5: serve once the acquisition snapshot is durable.
ClaimServe(p, s) ==
    /\ Running(p)
    /\ srv[p][s].st = "acqwait"
    /\ srv[p][s].x \in snaps[s]
    /\ srv'     = [srv EXCEPT ![p][s].st = "serving"]
    /\ durSnap' = [durSnap EXCEPT ![p][s] = [seq |-> Len(srv[p][s].hist), q |-> srv[p][s].x.q]]
    /\ UNCHANGED <<bucketVars, flightVars, pst, buf, wpos, pend, ackedPos, snapQ, spl,
                   ghostVars>>

\* §8.5 steps 1-3: stop the shard, wait until all its records are durable (a sync, so a
\* lease check), PUT the final snapshot.
ReleaseFinal(p, s) ==
    /\ Running(p)
    /\ srv[p][s].st = "serving"
    /\ ~Unacked(p, s)
    /\ IF LiveLease(p)
       THEN LET fin == FinalSnap(srv[p][s].gen, srv[p][s].hist) IN
            /\ snapPuts' = snapPuts \cup {[s |-> s, x |-> fin]}
            /\ srv' = [srv EXCEPT ![p][s].st = "relwait", ![p][s].x = fin]
            /\ UNCHANGED <<bucketVars, segPuts, pst, buf, wpos, pend, ackedPos, snapQ,
                           durSnap, spl, ghostVars>>
       ELSE Poison(p)

\* §8.5 steps 4-5: once the final snapshot is durable, CAS the owner record to released.
ReleaseCAS(p, s) ==
    /\ Running(p)
    /\ srv[p][s].st = "relwait"
    /\ srv[p][s].x \in snaps[s]
    /\ IF owner[s] = [n |-> p[1], i |-> p[2], gen |-> srv[p][s].gen, st |-> "owned"]
       THEN /\ owner'   = [owner EXCEPT ![s].st = "released"]
            /\ srv'     = [srv EXCEPT ![p][s] = NoSrv]
            /\ durSnap' = [durSnap EXCEPT ![p][s] = NoDur]
            /\ UNCHANGED <<lease, wm, log, snaps, marker, flightVars, pst, buf, wpos, pend,
                           ackedPos, snapQ, spl, ghostVars>>
       ELSE Poison(p)

\* --------------------------------------------------------------- splits --

\* §8.3, run by any node for a fenced incarnation d.
SplitBegin(d, k) ==
    /\ IncDead(d)
    /\ d \notin marker
    /\ spl[d][k].st = "idle"
    /\ NeedsSplit(d)
    /\ spl' = [spl EXCEPT ![d][k] = [IdleSplit EXCEPT !.st = "listing", !.cur = 1]]
    /\ UNCHANGED <<bucketVars, flightVars, procVars, ghostVars>>

\* LIST, one key at a time: a LIST is linearizable per key but is not an atomic snapshot
\* of the prefix, so concurrent deletes and landings can show through in any mix.
SplitList(d, k) ==
    /\ spl[d][k].st = "listing"
    /\ spl[d][k].cur <= MaxPos
    /\ spl' = [spl EXCEPT ![d][k].seen[spl[d][k].cur] = log[d][spl[d][k].cur].k,
                          ![d][k].cur = @ + 1]
    /\ UNCHANGED <<bucketVars, flightVars, procVars, ghostVars>>

\* Choose where reading starts and ends, and seal the end (create-if-absent). If a late
\* PUT just took the position, list again.
\*   draft:     start at the lowest listed position (PHASE2 as first written)
\*   maxlow:    start at max(watermark, lowest listed position)
\*   watermark: start at the watermark
SplitSeal(d, k) ==
    LET sp   == spl[d][k]
        lo   == IF \E q \in Pos : sp.seen[q] # "absent"
                THEN Min({q \in Pos : sp.seen[q] # "absent"}) ELSE 1
        from == CASE SplitRule = "draft"     -> lo
                  [] SplitRule = "maxlow"    -> Max(wm[d], lo)
                  [] SplitRule = "watermark" -> wm[d]
        e    == Min({q \in from..MaxPos : sp.seen[q] # "data"})
    IN
    /\ sp.st = "listing"
    /\ sp.cur > MaxPos
    /\ CASE log[d][e].k = "absent" ->
              /\ log' = [log EXCEPT ![d][e] = Sealed]
              /\ spl' = [spl EXCEPT ![d][k] = [sp EXCEPT !.st = "reading", !.start = from,
                                                         !.end = e, !.cur = from]]
         [] log[d][e].k = "sealed" ->
              /\ spl' = [spl EXCEPT ![d][k] = [sp EXCEPT !.st = "reading", !.start = from,
                                                         !.end = e, !.cur = from]]
              /\ UNCHANGED log
         [] OTHER ->
              /\ spl' = [spl EXCEPT ![d][k] = IdleSplit]
              /\ UNCHANGED log
    /\ UNCHANGED <<lease, wm, owner, snaps, marker, flightVars, procVars, ghostVars>>

\* Read segments up to the seal, one at a time. A sealed position ends the log. A segment
\* that vanished is skipped (draft: the split starts over).
SplitRead(d, k) ==
    LET sp == spl[d][k]
        c  == log[d][sp.cur]
    IN
    /\ sp.st = "reading"
    /\ sp.cur < sp.end
    /\ CASE c.k = "data"   -> spl' = [spl EXCEPT ![d][k].got = @ \o c.c, ![d][k].cur = @ + 1]
         [] c.k = "sealed" -> spl' = [spl EXCEPT ![d][k].end = sp.cur]
         [] OTHER          -> spl' = IF SplitRule = "draft"
                                     THEN [spl EXCEPT ![d][k] = IdleSplit]
                                     ELSE [spl EXCEPT ![d][k].cur = @ + 1]
    /\ UNCHANGED <<bucketVars, flightVars, procVars, ghostVars>>

\* After reading: for each (shard, gen) seen, load that shard's newest snapshot at that
\* gen and apply the records above it, in log order. PUT a split snapshot if any.
SplitOf(s, g, got) ==
    LET base == CHOOSE x \in MaximalAt(s, g) : TRUE
        new  == SelectSeq(got, LAMBDA id : rec[id].shard = s /\ rec[id].gen = g
                                           /\ rec[id].seq > Len(base.hist))
    IN [s |-> s, x |-> SplitSnap(g, base.hist \o new), new |-> new]

SplitWrite(d, k) ==
    LET sp     == spl[d][k]
        groups == {<<rec[id].shard, rec[id].gen>> : id \in Range(sp.got)}
        outs   == {SplitOf(gr[1], gr[2], sp.got) : gr \in {g2 \in groups : MaximalAt(g2[1], g2[2]) # {}}}
        xs     == {[s |-> o.s, x |-> o.x] : o \in {o2 \in outs : o2.new # <<>>}}
    IN
    /\ sp.st = "reading"
    /\ sp.cur >= sp.end
    /\ snapPuts' = snapPuts \cup xs
    /\ spl' = [spl EXCEPT ![d][k].st = "written", ![d][k].xs = xs]
    /\ UNCHANGED <<bucketVars, segPuts, procVars, ghostVars>>

\* Once every split snapshot is durable: splits/{node}/{inc}.
SplitMark(d, k) ==
    /\ spl[d][k].st = "written"
    /\ \A u \in spl[d][k].xs : u.x \in snaps[u.s]
    /\ marker' = marker \cup {d}
    /\ spl'    = [spl EXCEPT ![d][k] = IdleSplit]
    /\ UNCHANGED <<lease, wm, owner, log, snaps, flightVars, procVars, ghostVars>>

\* A splitter crashes; another one starts over later.
SplitAbort(d, k) ==
    /\ spl[d][k].st # "idle"
    /\ spl' = [spl EXCEPT ![d][k] = IdleSplit]
    /\ UNCHANGED <<bucketVars, flightVars, procVars, ghostVars>>

\* ------------------------------------------------------------------- GC --

\* §8.7: a split incarnation's log may go, in any order.
DeadLogDelete(d, q) ==
    /\ d \in marker
    /\ log[d][q].k # "absent"
    /\ log' = [log EXCEPT ![d][q] = Absent]
    /\ UNCHANGED <<lease, wm, owner, snaps, marker, flightVars, procVars, spl, ghostVars>>

\* §8.7: a split marker goes once nothing could need it again.
MarkerGC(d) ==
    /\ d \in marker
    /\ ~NeedsSplit(d)
    /\ marker' = marker \ {d}
    /\ UNCHANGED <<lease, wm, owner, log, snaps, flightVars, procVars, spl, ghostVars>>

\* ------------------------------------------------------------------ next --

Next ==
    \/ \E n \in Node : Start(n) \/ Tombstone(n)
    \/ \E p \in Proc :
          \/ Crash(p) \/ Flush(p) \/ Commit(p) \/ TruncMark(p) \/ TruncDel(p)
          \/ \E e \in snapQ[p] : SnapSync(p, e)
          \/ \E s \in Shard :
                \/ Append(p, s) \/ SnapCapture(p, s)
                \/ ClaimCAS(p, s) \/ ClaimLoad(p, s) \/ ClaimServe(p, s)
                \/ ReleaseFinal(p, s) \/ ReleaseCAS(p, s)
    \/ \E u \in segPuts : LandSeg(u)
    \/ \E u \in snapPuts : SnapLand(u)
    \/ \E s \in Shard : \E x \in snaps[s] : PruneSnap(s, x)
    \/ \E d \in Proc :
          \/ MarkerGC(d)
          \/ \E q \in Pos : DeadLogDelete(d, q)
          \/ \E k \in Splitters :
                \/ SplitBegin(d, k) \/ SplitList(d, k) \/ SplitSeal(d, k)
                \/ SplitRead(d, k) \/ SplitWrite(d, k) \/ SplitMark(d, k) \/ SplitAbort(d, k)

Spec == Init /\ [][Next]_vars

Symmetry == Permutations(Node) \cup Permutations(Shard)

\* ------------------------------------------------------------ invariants --

Loaded(p, s) == srv[p][s].st \in {"acqwait", "serving", "relwait"}
RecOf(id) == rec[id]

\* Every history anyone holds or stores is well formed: the record at index j has
\* shard_seq j, so replay never skips or reorders a shard's records.
WellFormed ==
    /\ \A s \in Shard : \A x \in snaps[s] :
          \A j \in 1..Len(x.hist) : rec[x.hist[j]].shard = s /\ rec[x.hist[j]].seq = j
    /\ \A p \in Proc : \A s \in Shard : Loaded(p, s) =>
          \A j \in 1..Len(srv[p][s].hist) :
              rec[srv[p][s].hist[j]].shard = s /\ rec[srv[p][s].hist[j]].seq = j

\* G1: a shard loaded at generation g holds every acknowledged record of an older
\* generation, at its shard_seq.
AckedDurable ==
    \A p \in Proc : \A s \in Shard : Loaded(p, s) =>
        \A id \in acked :
            (rec[id].shard = s /\ rec[id].gen < srv[p][s].gen) =>
                /\ rec[id].seq <= Len(srv[p][s].hist)
                /\ srv[p][s].hist[rec[id].seq] = id

\* G2: one history per shard. No two acknowledged records share a (shard, shard_seq);
\* every stored or loaded history agrees with every acknowledged record it reaches;
\* snapshots of one generation are prefixes of each other.
Agrees(h, g, s) ==
    \A id \in acked :
        (rec[id].shard = s /\ rec[id].gen <= g /\ rec[id].seq <= Len(h)) => h[rec[id].seq] = id

IsPrefix(a, b) == Len(a) <= Len(b) /\ \A j \in 1..Len(a) : a[j] = b[j]

LinearHistory ==
    /\ \A i, j \in acked :
          (rec[i].shard = rec[j].shard /\ rec[i].seq = rec[j].seq) => i = j
    /\ \A s \in Shard : \A x \in snaps[s] : Agrees(x.hist, x.gen, s)
    /\ \A p \in Proc : \A s \in Shard : Loaded(p, s) => Agrees(srv[p][s].hist, srv[p][s].gen, s)
    /\ \A s \in Shard : \A x, y \in snaps[s] :
          x.gen = y.gen => IsPrefix(x.hist, y.hist) \/ IsPrefix(y.hist, x.hist)

\* A live incarnation's log is never sealed.
NoLiveLogSealed ==
    \A n \in Node : lease[n].st = "alive" =>
        \A q \in Pos : log[<<n, lease[n].inc>>][q].k # "sealed"

\* GC safety: from the bucket alone, every acknowledged record is recoverable. It is in
\* each newest snapshot of its shard, or that snapshot points into the record's own log
\* below it and every segment from the pointer to the record is still there.
InLogFrom(id, x) ==
    LET d == <<x.n, x.i>> IN
    /\ x.k = "node"
    /\ rec[id].n = x.n /\ rec[id].i = x.i /\ rec[id].gen = x.gen
    /\ \E q2 \in Pos :
          /\ q2 >= x.q
          /\ log[d][q2].k = "data" /\ id \in Range(log[d][q2].c)
          /\ \A q3 \in x.q..q2 : log[d][q3].k = "data"

GCSafety ==
    \A id \in acked :
        LET s == rec[id].shard IN
        /\ snaps[s] # {}
        /\ \A x \in Maximal(s) :
              \/ rec[id].seq <= Len(x.hist) /\ x.hist[rec[id].seq] = id
              \/ InLogFrom(id, x)

\* ------------------------------------------------------------- liveness --

\* Weak fairness on everything but faults: no crash, no tombstone of a running process,
\* no splitter abort, no prune requirement. LiveModel restricts tombstones to crashed
\* incarnations, so the guardian is fair but never wrong.
Fairness ==
    /\ \A n \in Node : WF_vars(Tombstone(n))
    /\ \A p \in Proc : WF_vars(Flush(p)) /\ WF_vars(Commit(p))
    /\ \A p \in Proc : \A s \in Shard :
          WF_vars(ClaimCAS(p, s)) /\ WF_vars(ClaimLoad(p, s)) /\ WF_vars(ClaimServe(p, s))
    /\ \A u \in [n : Node, i : Inc, q : Pos, c : Seq(1..MaxRec)] : WF_vars(LandSeg(u) /\ u \in segPuts)
    /\ \A d \in Proc : \A k \in Splitters :
          /\ WF_vars(SplitBegin(d, k)) /\ WF_vars(SplitList(d, k)) /\ WF_vars(SplitSeal(d, k))
          /\ WF_vars(SplitRead(d, k)) /\ WF_vars(SplitWrite(d, k)) /\ WF_vars(SplitMark(d, k))

=============================================================================
