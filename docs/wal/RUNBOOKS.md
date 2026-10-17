# Valka runbooks

What to do when an alert from `deploy/prometheus/alerts.yml` fires. Background: the bucket is
the only authority. Each node holds a lease (`nodes/{node}/lease`) it renews every 2 s; each
shard has an owner record (`owners/{hh}/{shard}`) naming one node incarnation and a generation.
A node whose lease stops changing for 10 s is declared dead by a guardian node, which splits
its log into per-shard snapshots; other nodes then claim its shards. See
[`ARCHITECTURE.md`](../ARCHITECTURE.md) for the full picture.

Useful endpoints on any node (`:8989`):

| Endpoint | Shows |
|---|---|
| `GET /api/v1/cluster` | every node, its shard count, WAL state, health, unowned shards |
| `GET /api/v1/cluster/members` | this node's view of the leases: live, suspect, dead |
| `GET /api/v1/cluster/shards` | one row per shard: owner, generation, phase, tasks |
| `GET /api/v1/cluster/placement` | operator pins, eligible nodes, effective protocol |
| `GET /api/v1/cluster/audit` | admin actions (needs the admin token) |
| `GET /metrics` | Prometheus metrics |

Admin actions need `Authorization: Bearer $VALKA_ADMIN_TOKEN`.

---

## Node not serving

`valka_node_serving == 0`: the node has not renewed its lease for `lease_timeout -
lease_safety_margin` (7 s by default). It stops dispatching and answers `UNAVAILABLE` so that
nobody can be acting on its shards while another node takes them over.

1. Check the node's logs for `lease renewal failed` and the error (credentials, DNS, bucket
   throttling, network).
2. If the bucket is reachable again, the node resumes on its next successful renewal; nothing
   to do.
3. If it stays cut off for more than `lease_timeout` (10 s), its guardian tombstones it and
   its shards move. The node then learns it is fenced and exits (see
   [Node fenced](#node-fenced)).

## Bucket slow or down

`valka_lease_age_seconds` rising on all nodes at once means the bucket is slow or failing.

- Writes are acknowledged only after their WAL segment is in the bucket, so clients see
  higher latency or `UNAVAILABLE`, never lost acknowledged work.
- If the bucket is down for everyone, every node self-fences and nobody can tombstone anybody.
  The cluster recovers by itself when the bucket returns.
- A node whose WAL PUTs fail after retries poisons its writer and exits non-zero; its
  supervisor restarts it, and the restart splits what it left behind.

## Node fenced

`valka_node_fenced_total` increased: the node found its lease held by someone else, either a
guardian's tombstone (it was declared dead, typically after a long pause or a cut-off) or a
newer process started with the same node id.

1. Make sure the process restarted (a fenced node exits; the supervisor restarts it with a new
   incarnation).
2. If two processes share a node id (misconfiguration), they keep fencing each other: give
   each a distinct `VALKA_NODE_ID`. A process refuses to start while another live process
   holds its id (`startup_lease_check`).
3. Nothing acknowledged was lost: acks are released only after a lease check that a
   tombstone would fail.

## Node lost

`valka_tombstones_total` increased: a guardian declared a node dead.

1. `GET /api/v1/cluster/members`: the node shows `dead`, `dead_by` names the guardian.
2. Its shards come back within ~10–15 s: split (`valka_splits_total`), then claims
   (`valka_shard_claims_total{result="claimed"}`). Workers attached to the dead node reconnect
   to another node and reattach their running tasks (`valka_reattach_total`; `retrying` while
   a task's shard is still moving, `failed` only if its owner stayed unreachable for 60 s).
3. Replace or restart the machine. A restart with the same node id takes a new incarnation and
   simply joins again.

## Stuck split

`valka_split_failures_total` rising: a dead incarnation's log cannot be split, so its shards
cannot be claimed (a claim needs the split marker `splits/{node}/{inc}`).

1. Find the error in the guardian's logs (`split failed; will retry`).
2. Bucket errors: they clear by themselves; any node retries after `split_timeout`.
3. `split of shard … shard_seq jumps`: a gap in a shard's record sequence. This is a bug, never
   data to serve; the split refuses to write rather than serve a hole. Keep the log
   (`wal/{node}/`) and open an issue with the guardian's logs. The affected shards stay
   unavailable; the rest of the cluster is unaffected.
4. `corrupt segment`: a segment failed its CRC. Same as 3: keep the bucket as is and report.

## Unowned shards

`sum(valka_shards_owned)` below the shard count for more than two minutes.

1. `GET /api/v1/cluster/shards?unowned=true` lists the shards nobody serves.
2. Usually a split is in progress or stuck ([Stuck split](#stuck-split)), or no node is
   eligible (all draining, or every node younger than `join_delay` after a full restart).
3. `GET /api/v1/cluster/placement`: a pin to a node that is not eligible is ignored; check
   `eligible_nodes`.

## Claim invariant

`valka_shard_claims_total{result="invariant"}`: a claim found the bucket inconsistent with
the protocol (a snapshot at or above the generation being claimed, or a newest snapshot
pointing into a live log). The claim is refused, so nothing wrong is served.

1. Do not delete anything in the bucket.
2. Collect `GET /api/v1/cluster/shards/{shard}`, the shard's `owners/` object and its
   `snapshots/…/{shard}/` listing, and report it.

## Nodes cannot reach each other

`valka_forwarded_commands_total{result="unreachable"}` rising: nodes reach the bucket but not
each other's internal listener (`cluster.internal_addr`, default `:50052`).

- Requests for shards owned by an unreachable node get `UNAVAILABLE` (clients retry); nothing
  is taken over, because the lease keeps renewing through the bucket.
- Check firewalls/security groups for the internal port, `cluster.advertise_internal_addr`,
  the cluster token (a wrong token shows as `Unauthenticated` in the logs) and, with mTLS, the
  certificates' SANs (they must match the advertised host).

---

## Planned operations

**Rolling restart.** Restart one node at a time. SIGTERM drains a node when live peers can
take its shards (final snapshots, nothing to replay); a node alone keeps its shards and
reclaims them on restart.

**Drain a node** (decommission, maintenance):
`POST /api/v1/cluster/nodes/{id}/drain` — it hands every shard over, marks its lease dead and
exits.

**Move a shard:** `POST /api/v1/cluster/shards/{id}/move` with `{"to": "node"}` pins it;
`DELETE /api/v1/cluster/shards/{id}/pin` returns it to automatic placement.

**Grow a hot queue:** `PUT /api/v1/queues/{queue}/partitions` with `{"partitions": n}`. New
partitions take new tasks at once; keyed creates still find keys under their old partition,
so idempotency stays exact.

**Upgrade.** Restart nodes one at a time onto the new build. A node refuses to join a cluster
whose live nodes already speak a newer protocol (`effective_protocol` in
`/api/v1/cluster/placement`).
