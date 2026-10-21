# Changelog

All notable changes to Valka. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/) (before 1.0, a minor version may
change the storage format or the API).

## [0.2.0] - 2026-10-22

Valka no longer needs a database. Every node keeps its shards in memory and writes a
write-ahead log to an S3-compatible bucket; any number of nodes share one bucket. The
PostgreSQL implementation (0.1) lives on in the `legacy-postgres` branch.

### Storage and durability

- Write-ahead log on object storage: every mutation is validated in memory, appended to a
  group-committed segment (50 ms window by default), and acknowledged only once the segment
  is in the bucket and the node has re-read its own lease.
- Segments are written create-if-absent; a retried PUT counts only if the stored bytes are ours.
- Per-shard snapshots with exact replay (`shard_seq` and ownership generation), WAL truncation
  after each snapshot round, and a log budget that bounds what a node can accumulate.
- Backends: a local directory (development), S3 and any S3-compatible store, and memory (tests).
- A poisoned WAL writer stops the server with a non-zero exit code.

### Clustering

- One code path for one node or many: node leases with incarnations, shard owner records moved
  only by compare-and-swap, a lease check after every segment PUT, guardians that tombstone a
  dead node's lease and split its log so its shards can be claimed at once.
- Rendezvous placement with operator pins, join delay and minimum residency; drains that hand
  every shard over before a node stops.
- Any node takes any request and forwards it to the shard owner; lists and stats are gathered
  from every node. Queues have partitions (16 by default, resizable online).
- Workers on any node run tasks owned by any node; reconnecting workers reattach the tasks they
  are still running.
- Node-to-node calls need a cluster token; mutual TLS is optional.

### API and clients

- API keys for the public REST and gRPC APIs (`VALKA_API_KEYS`), a separate admin token for
  the admin API (drain, move and pin shards, queue partitions, audit log).
- Idempotent creates: a repeated idempotency key returns the original task (`200` and an
  `idempotent-replayed` header) instead of an error.
- Per-node limits on tasks held in memory and on payload sizes (`429` / `RESOURCE_EXHAUSTED`).
- Step checkpoints: a retry skips the steps a previous attempt finished.
- End-to-end result acknowledgements: workers keep each result until the server confirms it,
  and resend it after a reconnect.
- All four SDKs (Rust, TypeScript, Python, Go) accept several node addresses, read
  `VALKA_API_KEY`, and retry creates safely with generated idempotency keys.
- The CLI covers tasks, logs, workers, cluster status and members, drains, shard moves and
  queue partitions.

### Dashboard and docs

- Cluster pages: nodes, a 4096-shard map, storage, and an Operations page for drains, shard
  placement, queue partitions and the audit log. Sign-in with an API key.
- Architecture guide, protocol specifications, runbooks, and a documentation site.

### Verification

- A TLA+ model of the ownership protocol, checked in CI (seven protocol bugs found before the
  cluster code existed).
- A randomized cluster simulation with a history checker (kills, freezes, lost PUT answers),
  which found four bugs, each fixed with a regression test.
- A chaos script for the three-node compose cluster, and a load generator
  (`examples/rs/bench.rs`). Load tests found and fixed a feeder livelock, a per-worker limit of
  about 20 results a second, and offers that waited 30 s after a refused batch.

### Removed

- PostgreSQL and every other external service. The `valka-db` and `valka-scheduler` crates are
  gone; their work is done by the engine.

## [0.1.0]

The first version, built on PostgreSQL. See the `legacy-postgres` branch.

[0.2.0]: https://github.com/IWhitebird/Valka/releases/tag/v0.2.0
[0.1.0]: https://github.com/IWhitebird/Valka/tree/legacy-postgres
