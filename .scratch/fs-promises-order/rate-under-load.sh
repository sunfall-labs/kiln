#!/usr/bin/env bash
# Same as rate.sh, but with the machine's 16 cores saturated first, since the flake is a
# wall-clock race between a due `setTimeout(0)` and a Tokio blocking-pool filesystem write.
set -u
n="${1:-30}"
jobs="${2:-16}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root" || exit 1
pids=()
for _ in $(seq 1 "$jobs"); do
  ( while :; do :; done ) &
  pids+=($!)
done
trap 'kill "${pids[@]}" 2>/dev/null' EXIT
bash .scratch/fs-promises-order/rate.sh "$n"
