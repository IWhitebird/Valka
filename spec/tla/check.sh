#!/usr/bin/env bash
# Model-check configurations of Ownership.tla with TLC.
#
#   spec/tla/check.sh                          every top-level *.cfg must pass
#   spec/tla/check.sh Restart.cfg              just these
#   spec/tla/check.sh --expect-fail counterexamples/*.cfg
#                                              each must end in a property violation
#
# TLA2TOOLS points at tla2tools.jar (default: ./tla2tools.jar next to this script).
# TLC_WORKERS defaults to "auto".
set -euo pipefail
cd "$(dirname "$0")"

jar=${TLA2TOOLS:-tla2tools.jar}
workers=${TLC_WORKERS:-auto}
expect_fail=0
if [[ ${1:-} == --expect-fail ]]; then
  expect_fail=1
  shift
fi
cfgs=("$@")
if [[ ${#cfgs[@]} -eq 0 ]]; then
  cfgs=(*.cfg)
fi

status=0
for cfg in "${cfgs[@]}"; do
  meta=$(mktemp -d)
  log=$(mktemp)
  start=$(date +%s)
  set +e
  java -XX:+UseParallelGC -cp "$jar" tlc2.TLC -config "$cfg" -workers "$workers" \
    -deadlock -nowarning -metadir "$meta" Ownership.tla >"$log" 2>&1
  code=$?
  set -e
  secs=$(( $(date +%s) - start ))
  summary=$({ grep -E "distinct states found|violated" "$log" || true; } | tail -2 | tr '\n' ' ')
  # TLC exits 12 on a safety violation and 13 on a liveness violation.
  if [[ $expect_fail -eq 1 ]]; then
    if [[ $code -eq 12 || $code -eq 13 ]]; then
      echo "ok    $cfg fails as expected (${secs}s): $summary"
    else
      echo "FAIL  $cfg was expected to violate a property, TLC exited $code (${secs}s)"
      tail -40 "$log"
      status=1
    fi
  elif [[ $code -eq 0 ]]; then
    echo "ok    $cfg (${secs}s): $summary"
  else
    echo "FAIL  $cfg: TLC exited $code (${secs}s)"
    tail -200 "$log"
    status=1
  fi
  rm -rf "$meta" "$log"
done
exit $status
