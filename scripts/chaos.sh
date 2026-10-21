#!/usr/bin/env bash
# Chaos test against the three-node compose cluster (docker-compose.cluster.yml).
#
# While a load generator (examples/rs/chaos.rs) creates tasks through every node, runs a
# worker on every node and records every acknowledged create, this script keeps breaking
# nodes: kill -9, freeze (SIGSTOP, like a long GC pause or a VM freeze), SIGTERM restart.
# At the end every node comes back and the load generator checks that every acknowledged
# task exists, finished, and that no idempotency key produced two tasks.
#
#   scripts/chaos.sh                       # 5 minutes of chaos
#   DURATION=1800 scripts/chaos.sh         # a 30-minute soak
#   NO_BUILD=1 scripts/chaos.sh            # reuse the valka:local image
#
# Needs docker compose and cargo. Exit status: 0 when every check passes.
set -euo pipefail
cd "$(dirname "$0")/.."

DURATION=${DURATION:-300}
RATE=${RATE:-20}
COMPOSE=(docker compose -f docker-compose.cluster.yml)
NODES=(valka-1 valka-2 valka-3)
LOG_DIR=${LOG_DIR:-target/chaos}
# The compose cluster's API key (docker-compose.cluster.yml); the SDK reads it.
export VALKA_API_KEY=${VALKA_API_KEY:-change-me-api-key}
mkdir -p "$LOG_DIR"

log() { echo "[chaos $(date +%H:%M:%S)] $*"; }

if [[ -z ${NO_BUILD:-} ]]; then
  log "building the image (CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4})"
  "${COMPOSE[@]}" build --build-arg "CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}" valka-1
fi
log "starting the cluster"
"${COMPOSE[@]}" up -d
for port in 8981 8982 8983; do
  for _ in $(seq 1 120); do
    curl -sf "http://localhost:$port/healthz" >/dev/null && break
    sleep 1
  done
done
log "cluster up: $(curl -s -H "Authorization: Bearer $VALKA_API_KEY" http://localhost:8981/api/v1/cluster | head -c 300)"

log "building the load generator"
cargo build -p valka-examples --example chaos >/dev/null
LOAD_SECONDS=$((DURATION + 20))
VALKA_CHAOS_SECONDS=$LOAD_SECONDS VALKA_CHAOS_RATE=$RATE VALKA_CHAOS_DRAIN=240 \
  VALKA_CHAOS_OUT="$LOG_DIR/acked.txt" \
  target/debug/examples/chaos >"$LOG_DIR/load.log" 2>&1 &
LOAD=$!

restore() {
  for n in "${NODES[@]}"; do
    "${COMPOSE[@]}" unpause "$n" >/dev/null 2>&1 || true
    "${COMPOSE[@]}" start "$n" >/dev/null 2>&1 || true
  done
}
trap restore EXIT

sleep 15
end=$((SECONDS + DURATION))
kills=0 pauses=0 restarts=0
while (( SECONDS < end )); do
  node=${NODES[$((RANDOM % ${#NODES[@]}))]}
  case $((RANDOM % 3)) in
    0)
      log "kill -9 $node"
      "${COMPOSE[@]}" kill -s SIGKILL "$node" >/dev/null
      kills=$((kills + 1))
      sleep $((5 + RANDOM % 15))
      "${COMPOSE[@]}" start "$node" >/dev/null
      ;;
    1)
      log "freeze $node"
      "${COMPOSE[@]}" pause "$node" >/dev/null
      pauses=$((pauses + 1))
      sleep $((5 + RANDOM % 20))
      "${COMPOSE[@]}" unpause "$node" >/dev/null
      log "thaw $node"
      ;;
    2)
      log "restart (SIGTERM) $node"
      "${COMPOSE[@]}" restart -t 30 "$node" >/dev/null
      restarts=$((restarts + 1))
      ;;
  esac
  sleep $((10 + RANDOM % 20))
done
restore
log "chaos over: $kills kills, $pauses freezes, $restarts restarts; waiting for the checker"
status=0
wait "$LOAD" || status=$?
cat "$LOG_DIR/load.log"
curl -s -H "Authorization: Bearer $VALKA_API_KEY" http://localhost:8981/api/v1/cluster \
  > "$LOG_DIR/cluster.json" || true
"${COMPOSE[@]}" logs --no-color > "$LOG_DIR/cluster.log" 2>&1 || true
log "logs in $LOG_DIR; exit $status"
exit $status
