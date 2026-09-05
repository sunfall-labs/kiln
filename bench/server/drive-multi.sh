#!/usr/bin/env bash
# Several client processes at once, so a single-threaded server is what saturates rather than a
# single-threaded client: at ~85k rps one node client process is itself a full core.
# usage: bench/server/drive-multi.sh <processes> <connections-per-process> [durationMs=5000] [port=18088] [workload=hello|mixed]
set -euo pipefail
procs="${1:?processes}"; conns="${2:?connections per process}"; duration="${3:-5000}"; port="${4:-18088}"; workload="${5:-hello}"
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
for i in $(seq 1 "$procs"); do
	if [[ $workload == mixed ]]; then
		node "$HERE/drive-mix.mjs" "$duration" "$conns" "$port" > "$tmp/$i.json" &
	else
		node "$HERE/drive.mjs" "$duration" "$conns" "$port" "$workload" > "$tmp/$i.json" &
	fi
done
wait
python3 - "$tmp" "$procs" "$conns" <<'PY'
import json,sys,glob
files=sorted(glob.glob(sys.argv[1]+"/*.json")); rows=[json.load(open(f)) for f in files]
rps=sum(r["rps"] for r in rows); errors=sum(r["errors"] for r in rows); wrong=sum(r.get("wrong",0) for r in rows)
p50=sorted(r["p50_ms"] for r in rows)[len(rows)//2]; p99=max(r["p99_ms"] for r in rows)
print(json.dumps({"processes":int(sys.argv[2]),"connections":int(sys.argv[2])*int(sys.argv[3]),"rps":rps,"errors":errors,"wrong":wrong,"p50_ms":p50,"p99_max_ms":p99}))
PY
rm -rf "$tmp"
