#!/usr/bin/env bash
# Several client processes at once, so a single-threaded server is what saturates rather than a
# single-threaded client: at ~85k rps one node client process is itself a full core.
# usage: bench/server/drive-multi.sh <processes> <connections-per-process> [durationMs=5000] [port=18088] [workload=hello|mixed]
set -euo pipefail
procs="${1:?processes}"; conns="${2:?connections per process}"; duration="${3:-5000}"; port="${4:-18088}"; workload="${5:-hello}"
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
pids=()
results_dir="${DRIVE_RESULTS_DIR:-}"
if [[ -n $results_dir ]]; then mkdir -p "$results_dir"; fi
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT
for i in $(seq 1 "$procs"); do
	if [[ $workload == mixed ]]; then
		node "$HERE/drive-mix.mjs" "$duration" "$conns" "$port" > "$tmp/$i.json" &
	else
		node "$HERE/drive.mjs" "$duration" "$conns" "$port" "$workload" > "$tmp/$i.json" &
	fi
	pids+=("$!")
done
status=0
for pid in "${pids[@]}"; do
	wait "$pid" || status=1
done
if [[ -n $results_dir ]]; then
	cp "$tmp"/*.json "$results_dir"/
fi
python3 - "$tmp" "$procs" "$conns" <<'PY'
import json,sys,glob
files=sorted(glob.glob(sys.argv[1]+"/*.json")); rows=[json.load(open(f)) for f in files]
if len(rows) != int(sys.argv[2]):
    raise SystemExit(f"expected {sys.argv[2]} client results, got {len(rows)}")
if any(r.get("requests", 0) <= 0 for r in rows):
    raise SystemExit("one or more client workers completed without a request")
rps=sum(r["rps"] for r in rows); errors=sum(r["errors"] for r in rows); wrong=sum(r.get("wrong",0) for r in rows)
p50_values=[r["p50_ms"] for r in rows if r["p50_ms"] is not None]
p99_values=[r["p99_ms"] for r in rows if r["p99_ms"] is not None]
p50=sorted(p50_values)[len(p50_values)//2] if p50_values else None
p99=max(p99_values) if p99_values else None
print(json.dumps({"processes":int(sys.argv[2]),"connections":int(sys.argv[2])*int(sys.argv[3]),"rps":rps,"errors":errors,"wrong":wrong,"p50_ms":p50,"p99_max_ms":p99}))
PY
exit "$status"
