# Cluster management UI — plan

Status: **"buildable now" column shipped** (single-node observability); phase 2 items pending
the ownership protocol. Companion to `DESIGN.md` (architecture) and `PLAN.md` (phases).

Shipped: `GET /api/v1/cluster`, `/cluster/shards[?node&dirty&min_tasks]`, `/cluster/shards/:id`,
`/cluster/storage`, `POST /cluster/snapshot`; UI routes `/cluster`, `/cluster/nodes/:id`,
`/cluster/shards`, `/cluster/storage`; dashboard cluster strip; Node column on Workers and
task runs; Shard row on task detail. Tests: 5 REST integration tests, engine stats test,
writer backlog test, 20 web unit/component tests (vitest + Testing Library).

## 0. Why this matters more now

With PostgreSQL, a node was stateless glue and the interesting state lived in the
database. With the WAL architecture every node *owns* shards, holds their state in RAM,
writes its own log, and can be killed or replaced at any time. Operators now need to see
and act on things that never existed before:

- which node owns which shards, and whether every shard has exactly one live owner;
- how far each node's WAL is behind (unflushed records, oldest unacked write);
- when each shard was last snapshotted and how much log a recovery would replay;
- whether a node's writer is poisoned (bucket unreachable) and must restart;
- membership: who is alive, suspect, dead, joining, draining;
- storage: how many segments/snapshots exist, how much the bucket costs per day.

Today the dashboard has Tasks / Workers / Events / Dead Letters. None of the above is
visible anywhere except logs and `/api/v1/cluster`, which returns six fields.

## 1. What ships now vs. with phase 2

| Capability | Buildable now (single owner) | Needs phase 2 (ownership protocol) |
|---|---|---|
| Node card: id, epoch, uptime, version, storage backend, durable LSN, unflushed records, poisoned | ✅ | |
| Shard heatmap: owner, task counts, snapshot age, records since snapshot | ✅ (one owner) | multi-owner coloring |
| Storage panel: segments, snapshots, bytes, truncation lag, est. request cost | ✅ | |
| Workers table "Node" column; task detail shows shard + owner node | ✅ | cross-node link |
| Cluster health banner (unowned shards, poisoned writer, suspect nodes) | ✅ (partial) | membership signals |
| Membership list from gossip with alive / suspect / dead | | ✅ |
| Takeover timeline, epoch history per shard | | ✅ |
| Actions: drain node, force takeover, move shard, rebalance | | ✅ |
| Cluster-wide task list / counts (scatter-gather) | | ✅ |
| Cluster events SSE (NodeJoined/Left, ShardsMoved, TakeoverDone, WriterPoisoned) | partial (poisoned) | ✅ |

Recommendation: build the "now" column first. It gives the observability the WAL design
needs even with one node, and every component is reused unchanged when nodes multiply.

## 2. Backend API additions

All under `/api/v1/cluster`. Read endpoints are open like the rest of the API; mutating
endpoints require `Authorization: Bearer <VALKA_ADMIN_TOKEN>` (new config, empty = disabled).

### Read

```
GET /api/v1/cluster
{
  "cluster_id", "this_node", "num_shards": 4096,
  "health": { "status": "ok|degraded|critical",
              "unowned_shards": 0, "poisoned_nodes": [], "suspect_nodes": [] },
  "nodes": [ ClusterNode ]            # phase 1: exactly one
}

ClusterNode {
  "node_id", "epoch", "status": "alive|suspect|dead|draining",
  "grpc_addr", "http_addr", "version", "started_at",
  "shards_owned": 4096,
  "tasks": { "pending", "running", "retry", "terminal_in_ram" },
  "workers_connected", "queues": [...],
  "wal": { "durable_lsn", "next_lsn", "unflushed_records", "oldest_unacked_ms",
           "flush_interval_ms", "poisoned": null | "reason" },
  "snapshots": { "last_round_at", "dirty_shards", "oldest_dirty_since_lsn" }
}

GET /api/v1/cluster/shards?node=&queue=&min_pending=
[ { "shard", "owner", "epoch", "tasks", "pending", "running",
    "records_since_snapshot", "snapshot_lsn", "snapshot_age_secs" } ]   # 4096 rows, ~300 KB
GET /api/v1/cluster/shards/:id            # one shard + its queues + last N record kinds

GET /api/v1/cluster/storage               # cached 30 s; one LIST per prefix
{ "backend", "bucket", "prefix",
  "wal":       { "segments", "bytes", "oldest_lsn", "newest_lsn", "per_node": {...} },
  "snapshots": { "count", "bytes", "shards_with_snapshot", "oldest_age_secs" },
  "logs":      { "chunks", "bytes" },
  "estimate":  { "puts_per_day", "usd_per_day" } }      # from flush interval + node count

GET /api/v1/cluster/events                # SSE: membership + ownership + writer health
```

### Write (phase 2, token-guarded)

```
POST /api/v1/cluster/snapshot             # run a snapshot round now (works in phase 1)
POST /api/v1/cluster/nodes/:id/drain      # stop taking shards, hand off, then exit
POST /api/v1/cluster/nodes/:id/evict      # force takeover of a dead node's shards
POST /api/v1/cluster/shards/:id/move      # { "to": node_id }
POST /api/v1/cluster/rebalance            # even out shards across alive nodes
```

Every admin action is appended to `admin/{ts}-{uuid}.json` in the bucket as an audit
record (who, what, before/after assignment version).

### Engine support required

- `Engine::stats() -> NodeStats` (task counts per status, per shard; dirty shards;
  oldest `dirty_since_lsn`) — one pass over shards, cheap.
- `WalWriter::pending_records()` and `oldest_unacked()` counters.
- `Store::stats(prefix)` helper for the storage panel.
- Phase 2: `ClusterManager` exposes membership with liveness state; `Ownership` exposes
  assignment history (`assignment` object versions).

### Cluster-wide reads (phase 2)

The dashboard talks to one node. Once shards are spread, `GET /tasks` on node A only sees
A's shards. Plan: add `InternalService.ListLocalTasks / CountLocalTasks`; the REST handler
fans out to alive peers with a 500 ms budget, merges, and sets `X-Valka-Partial: true`
when a peer timed out. `GET /tasks/:id` needs no fan-out: the shard is in the id, the
owner is in the assignment, so it is one forwarded call.

## 3. UI

New nav item **Cluster** with three routes, plus additions to existing pages.

### `/cluster` — topology
- Health banner: green "4096/4096 shards owned · 1 node · WAL current", or red with the
  specific problem (unowned shards, poisoned writer, node suspect).
- Node cards grid: status dot, node id, epoch badge, shards owned, workers, pending /
  running, durable LSN, unflushed count with a small sparkline, snapshot dirty count,
  uptime. Card click → node detail.
- Right column: recent cluster events (SSE), newest first.

### `/cluster/nodes/:id` — node detail
- Header: status, epoch, addresses, version, started_at, storage backend.
- WAL panel: durable vs next LSN, unflushed records, oldest unacked age, flush interval,
  poisoned reason if any.
- Snapshot panel: last round, dirty shards, oldest dirty LSN, "Snapshot now" button.
- Shards owned: compact range list ("0–1365, 2731–4095") with a link to the heatmap
  filtered to this node.
- Workers connected to this node (reuses `WorkerTable`).
- Phase 2 actions: Drain, Evict (confirm dialogs, token required).

### `/cluster/shards` — shard map
- 64 × 64 heatmap (one cell per shard). Color modes: *owner* (categorical per node),
  *pending load*, *snapshot age*, *records since snapshot*. Unowned shards hatched red.
- Hover tooltip: shard, owner, epoch, counts, snapshot age. Click → side drawer with the
  full shard record and a "Move to…" action (phase 2).
- Filters: node, queue (shards holding tasks of that queue), "only dirty", "only unowned".

### `/cluster/storage` — bucket
- Cards: segments (count, bytes, oldest age), snapshots (count, bytes, coverage), log
  chunks, estimated PUT/day and $/day.
- Per-node table of WAL size and truncation lag ("segments behind oldest snapshot").
- Retention settings read-only (completed_retention, snapshots_to_keep, flush interval).

### Changes to existing pages
- **Dashboard**: a slim cluster strip under the stats cards (nodes alive / total, shards
  owned / 4096, unflushed records, snapshot dirty shards) linking to `/cluster`.
- **Workers**: "Node" column (which node the stream is attached to).
- **Task detail**: "Shard" and "Owner node" rows in Details; runs table already shows
  `assigned_node_id`, make it a link to the node page.

### Frontend structure
```
web/src/api/cluster.ts            clusterApi.get / shards / shard / storage / subscribe
web/src/hooks/use-cluster.ts      polling (5 s) + SSE merge
web/src/pages/cluster.tsx, cluster-node.tsx, cluster-shards.tsx, cluster-storage.tsx
web/src/components/cluster/       node-card, health-banner, shard-heatmap (canvas, 4096
                                  cells), wal-panel, snapshot-panel, storage-cards,
                                  cluster-events
```
The heatmap is a `<canvas>` (4096 DOM nodes would be slow); tooltips via pointer math.
Charts stay dependency-free (existing Tailwind + Radix stack); no chart library needed for
sparklines (inline SVG).

## 4. Delivery order

1. **Backend read API** (`/cluster`, `/cluster/shards`, `/cluster/storage`), engine
   `stats()`, writer counters; integration tests for each endpoint. ~1 day.
2. **UI: Cluster page + Node detail + Storage** on the single node. ~1–2 days.
3. **Shard heatmap** + filters + drawer. ~1 day.
4. **Existing-page integrations** (dashboard strip, Node column, task detail rows). ~½ day.
5. **Phase 2 hooks**: membership from gossip, cluster events SSE, admin token + actions,
   scatter-gather lists. Lands with the ownership protocol work.

## 5. Open questions

- Should the shard heatmap poll (5 s, ~300 KB) or stream deltas? Start with polling; the
  payload compresses ~10× and 4096 rows is fine.
- Cost estimate constants (S3 PUT price) — config, default us-east-1.
- Admin auth: bearer token is the minimum; SSO/OIDC is out of scope for now.
