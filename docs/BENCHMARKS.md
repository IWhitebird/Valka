# Benchmarks

Measured on one laptop, so read the numbers as an order of magnitude and as a comparison
between setups, not as what a production cluster on S3 does. Every run is reproducible with
the commands below.

## Setup

| Item | Value |
|---|---|
| Machine | Intel Core i5-12450H (8 cores, 12 threads), 15 GB RAM, Linux 6.14, NVMe |
| Valka | 0.2.0, release build, default configuration (50 ms group commit, 4096 shards) |
| CPU caps | single node: 3 CPUs (`systemd-run -p CPUQuota=300%`); three nodes: 1.5 CPUs each; MinIO: 1.5 CPUs; load generator: 3 CPUs |
| Object store | local disk (`local` backend), or MinIO on the same machine (`pgsty/silo`) |
| Load | `examples/rs/bench.rs`: N creates in flight for 30 s, 100-byte inputs, one worker per node with 256 slots and a handler that returns at once |

The load generator reports the **create ack** (request sent → task durable and
acknowledged) and the **pickup** (create sent → the worker's handler starts), and checks
that every acknowledged task ran.

## Results

### One node, local disk

| In flight | Creates/s | Ack p50 | Ack p99 | Pickup p50 | Pickup p99 | Ran |
|---:|---:|---:|---:|---:|---:|---|
| 64 | 1,218 | 52.5 ms | 54.2 ms | 53.3 ms | 55.1 ms | all 36,544 |
| 256 | 4,669 | 54.7 ms | 60.6 ms | 59.5 ms | 81.6 ms | all 140,288 |
| 1,024 | 17,730 | 57.4 ms | 75.0 ms | 84.0 ms | 353 ms | all 532,940 |

A clean stop after each run (final snapshots of every dirty shard) took 0.4 s, 0.9 s and
3.0 s. The server held 533k tasks in 1.7 GB of RAM at the end of the largest run.

### Three nodes, MinIO

Producers spread over the nodes (each create may be forwarded to its shard's owner), one
worker per node; a task is run by whichever node's worker takes it first.

| In flight | Creates/s | Ack p50 | Ack p99 | Pickup p50 | Pickup p99 | Ran |
|---:|---:|---:|---:|---:|---:|---|
| 64 | 1,144 | 55.2 ms | 99.0 ms | 54.9 ms | 97.8 ms | all 34,390 |
| 256 | 4,589 | 54.5 ms | 101.4 ms | 54.8 ms | 104.4 ms | all 137,931 |
| 1,024 | 16,063 | 62.2 ms | 157.2 ms | 62.6 ms | 150.1 ms | all 483,645 |

### Failover (three nodes in Docker, MinIO)

`scripts/bench-failover.sh` kills a node with `kill -9` and polls another node for when the
dead node is **detected** (its lease tombstoned) and when the cluster has **recovered**
(every shard served by a live node again). Six trials, a different node each time:

| Of six trials | Detected | Recovered |
|---|---:|---:|
| fastest | 9.9 s | 13.5 s |
| median | 13.5 s | 22.3 s |
| slowest | 15.1 s | 26.7 s |

Detection is the lease timeout (10 s) plus up to one membership listing (3 s). Recovery
then takes one placement round and the claims themselves: about five object-store requests
per shard, here for ~1,400 shards per surviving node against a single MinIO limited to 1.5
CPUs. Workers keep running their tasks meanwhile and reattach them; creates and results for
the dead node's shards get `UNAVAILABLE` and the SDKs retry them.

## What bounds what

- **Ack latency** is the group-commit window (50 ms) plus one PUT and one conditional GET
  of the node's lease. `VALKA_WAL__FLUSH_INTERVAL_MS=10` gave 12 ms p50 acks on one node at
  64 in flight (5,222 creates/s), at five times the PUT count.
- **Throughput** is bounded by CPU (JSON encoding and compression of segments, gRPC), not by
  PUT latency: up to four segments are in flight per node and each holds every record of
  its window.
- **Pickup** is the ack latency plus about a millisecond while workers keep up: a task is
  handed to a waiting worker the moment it is durable.

## Object-store requests and cost

A busy node writes at most one segment per commit window, 20 per second at 50 ms, plus one
conditional GET of its lease per segment, one lease PUT every 2 s, one LIST of `nodes/` every
3 s, and snapshots of dirty shards every minute. An idle node writes only its lease. In the
single-node runs above the node wrote 525–572 segments in 30 s at every load: segments are
cut by time, not size, so the PUT count does not grow with throughput.

At the S3 Standard list price of $0.005 per 1,000 PUT requests, a node that is busy around
the clock writes about 1.7 million segments a day, about $8.60 a day; raising the window to
200 ms cuts that by four. The dashboard's storage page shows the estimate for your own
cluster.

## What the benchmarks found

The first runs found real bugs, each fixed with a regression test before these numbers
were taken:

| Symptom | Cause |
|---|---|
| A node under load stopped writing; even timers stopped | The feeder handed back a refused task, which woke it at once: a busy loop that never yielded |
| About 20 tasks/s per worker, whatever its concurrency | The server applied a worker's results one after another, each waiting for its group commit |
| Some tasks started 30 s late | A refused feeder batch dropped its other tasks until their offers lapsed |
| Workers fell behind as the backlog grew | Every task leaving PENDING scanned the whole pending index |
| Half of the tasks fetched from other nodes went back again | Dispatches to other owners ran one after another, with no slot open meanwhile |
| Recovery after `kill -9` took up to 37 s | Claims refused before the dead log was split backed off for up to 30 s |
| A clean stop with thousands of dirty shards took minutes | Snapshots were written one after another |

## Reproducing

```bash
cargo build --release -p valka-server -p valka-examples --example bench

# One node on a local directory
VALKA_STORAGE__PATH=/tmp/valka-bench ./target/release/valka-server &
VALKA_BENCH_INFLIGHT=256 VALKA_BENCH_WORKER_CONCURRENCY=256 ./target/release/examples/bench

# Three nodes
docker compose -f docker-compose.cluster.yml up -d --build
VALKA_API_KEY=change-me-api-key \
VALKA_BENCH_GRPC=http://localhost:50061,http://localhost:50062,http://localhost:50063 \
  ./target/release/examples/bench

# Failover
scripts/bench-failover.sh
```

`VALKA_BENCH_SECONDS`, `VALKA_BENCH_INFLIGHT`, `VALKA_BENCH_WORKER_CONCURRENCY` and
`VALKA_BENCH_PAYLOAD` change the load (see the top of `examples/rs/bench.rs`).
