#!/usr/bin/env bash
# Measure failover on the three-node compose cluster (docker-compose.cluster.yml).
#
# Each trial kill -9s one node and polls another node's REST API every 250 ms for:
#   detected   the killed node's lease is marked dead (by its guardian)
#   recovered  every shard is served again by a live node (no unowned shards)
# then restarts the node and waits for the cluster to settle before the next trial.
#
#   scripts/bench-failover.sh            # 5 trials
#   TRIALS=10 scripts/bench-failover.sh
#
# Needs the cluster up (docker compose -f docker-compose.cluster.yml up -d), curl and python3.
set -euo pipefail
cd "$(dirname "$0")/.."

TRIALS=${TRIALS:-5}
KEY=${VALKA_API_KEY:-change-me-api-key}
COMPOSE=(docker compose -f docker-compose.cluster.yml)
NODES=(valka-1 valka-2 valka-3)
PORTS=(8981 8982 8983)

now_ms() { date +%s%3N; }

# GET a JSON document from a node and evaluate a Python expression on it (`d`).
query() {
  local port=$1 path=$2 expr=$3
  curl -sf -m 2 -H "Authorization: Bearer $KEY" "http://localhost:$port$path" \
    | python3 -c "import json,sys; d=json.load(sys.stdin); print($expr)" 2>/dev/null || echo "?"
}

settled() {
  local port=$1
  [[ $(query "$port" /api/v1/cluster \
    "d['health']['unowned_shards']==0 and not d['health'].get('partial') and sum(1 for n in d['nodes'] if n['status']=='alive')==3") == "True" ]]
}

wait_settled() {
  local port=$1
  for _ in $(seq 1 240); do
    settled "$port" && return 0
    sleep 0.5
  done
  echo "the cluster did not settle" >&2
  return 1
}

printf "%-6s %-8s %12s %12s\n" "TRIAL" "KILLED" "DETECTED" "RECOVERED"
wait_settled 8981
for t in $(seq 1 "$TRIALS"); do
  i=$(( (t - 1) % 3 ))
  victim=${NODES[$i]}
  watch=${PORTS[$(( (i + 1) % 3 ))]}
  t0=$(now_ms)
  "${COMPOSE[@]}" kill -s SIGKILL "$victim" >/dev/null 2>&1
  detected="" recovered=""
  while [[ -z $recovered ]]; do
    sleep 0.25
    elapsed=$(( $(now_ms) - t0 ))
    if [[ -z $detected ]] && [[ $(query "$watch" /api/v1/cluster/members \
        "any(m['node_id']=='$victim' and m['status']=='dead' for m in d)") == "True" ]]; then
      detected=$elapsed
    fi
    if [[ -n $detected ]] && [[ $(query "$watch" /api/v1/cluster \
        "d['health']['unowned_shards']==0 and not d['health'].get('partial')") == "True" ]]; then
      recovered=$elapsed
    fi
    if (( elapsed > 120000 )); then
      recovered="timeout"
    fi
  done
  printf "%-6s %-8s %10s ms %10s ms\n" "$t" "$victim" "${detected:-?}" "$recovered"
  "${COMPOSE[@]}" start "$victim" >/dev/null 2>&1
  # The restarted node takes part in placement after its join delay (15 s).
  sleep 20
  wait_settled "$watch"
done
