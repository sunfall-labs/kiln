#!/usr/bin/env bash
#
# Server scenarios throughput and latency baseline across runtimes:
# node, bun, deno, and native kiln.
#
# Step B.0: Measure first (Server scenarios & leak check)
# Extended with scenarios:
#   - hello
#   - mixed
#   - static-file
#   - json-echo-1k
#   - json-echo-64k
#   - chunked-stream
#   - large-upload
#   - connection-churn
#   - fs-small-reads
#   - fs-large-read
#   - fs-stat-storm
#
# Baselined with the no-regression rule:
# Every row has a node, bun and deno column and a client count.
# A regression names the scenario.
#
# usage: scripts/server-baseline.sh [options]
#   --update              regenerate the baseline at bench/results/server-baseline.json
#   --compare <file>      baseline file to compare against (default: bench/results/server-baseline.json)
#   --threshold <pct>     regression threshold percentage (default: 15)
#   --clients <n>         client concurrency count (default: 16)
#   --duration-ms <ms>    duration per scenario run in ms (default: 2000)
#   --port <port>         port to use (default: 18088)
#   --out <file>          where to write the JSON results (default: bench/results/server-baseline.json when --update)
#   --kiln-bin <path>     kiln CLI binary (default: target/release/kiln)
#   --kiln-server <path>  prebuilt native server binary (optional)
#   --scenarios <list>    space-separated list of scenarios to run
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

OUT="bench/results/server-baseline.json"
COMPARE="bench/results/server-baseline.json"
UPDATE=0
THRESHOLD=15
CLIENTS=16
DURATION_MS=2000
PORT=18088
KILN_BIN="target/release/kiln"
KILN_SERVER=""
SCENARIOS="hello mixed static-file json-echo-1k json-echo-64k chunked-stream large-upload connection-churn fs-small-reads fs-large-read fs-stat-storm"

while [[ $# -gt 0 ]]; do
	case "$1" in
	--update)
		UPDATE=1
		shift
		;;
	--compare)
		COMPARE="${2:?--compare needs a path}"
		shift 2
		;;
	--threshold)
		THRESHOLD="${2:?--threshold needs a percentage}"
		shift 2
		;;
	--clients)
		CLIENTS="${2:?--clients needs a number}"
		shift 2
		;;
	--duration-ms)
		DURATION_MS="${2:?--duration-ms needs a number}"
		shift 2
		;;
	--port)
		PORT="${2:?--port needs a number}"
		shift 2
		;;
	--out)
		OUT="${2:?--out needs a path}"
		shift 2
		;;
	--kiln-bin)
		KILN_BIN="${2:?--kiln-bin needs a path}"
		shift 2
		;;
	--kiln-server)
		KILN_SERVER="${2:?--kiln-server needs a path}"
		shift 2
		;;
	--scenarios)
		SCENARIOS="${2:?--scenarios needs a list}"
		shift 2
		;;
	-h | --help)
		cat <<-'USAGE'
			usage: scripts/server-baseline.sh [options]

			  --update              regenerate the baseline file
			  --compare <file>      baseline file to compare against (default: bench/results/server-baseline.json)
			  --threshold <pct>     regression threshold percentage (default: 15)
			  --clients <n>         client concurrency count (default: 16)
			  --duration-ms <ms>    duration per scenario in ms (default: 2000)
			  --port <port>         port to use (default: 18088)
			  --out <file>          where to write the JSON results
			  --kiln-bin <path>     kiln CLI binary (default: target/release/kiln)
			  --kiln-server <path>  prebuilt native server binary
			  --scenarios <list>    scenarios to run
		USAGE
		exit 0
		;;
	*)
		echo "server-baseline.sh: unknown argument '$1' (try --help)" >&2
		exit 2
		;;
	esac
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/kiln-server-baseline.XXXXXX")"
cleanup_work() {
	rm -rf "$WORK"
}
trap cleanup_work EXIT

# Check / build kiln binary
if [[ ! -x "$KILN_BIN" ]]; then
	if [[ -x "target/debug/kiln" ]]; then
		KILN_BIN="target/debug/kiln"
	else
		echo "Building kiln CLI..."
		kiln_cargo build --release -p kiln-cli
		KILN_BIN="target/release/kiln"
	fi
fi

# Check / build native server binary
if [[ -z "$KILN_SERVER" || ! -x "$KILN_SERVER" ]]; then
	if [[ -x "/tmp/test-app-bin" ]]; then
		KILN_SERVER="/tmp/test-app-bin"
	else
		KILN_SERVER="$WORK/kiln-server-app"
		echo "Building native app server binary..."
		"$KILN_BIN" --one-shot build bench/server/app.mjs --node --profile native-release --output "$KILN_SERVER"
	fi
fi

# Pre-generate / verify benchmark fixtures
mkdir -p bench/server/fixtures/small
if [[ ! -f bench/server/fixtures/static.txt || ! -f bench/server/fixtures/large.bin || ! -f bench/server/fixtures/small/f00.txt ]]; then
	python3 -c '
with open("bench/server/fixtures/static.txt", "w") as f:
    f.write(("kiln static file benchmark content line\n" * 400)[:16384])
with open("bench/server/fixtures/large.bin", "wb") as f:
    f.write(b"KILN_LARGE_FILE_" * (1048576 // 16))
for i in range(20):
    with open(f"bench/server/fixtures/small/f{i:02d}.txt", "w") as f:
        f.write((f"small file {i:02d} " * 10)[:128])
'
fi

# Prepare server script copies with target PORT configured
sed "s/18088/${PORT}/g" bench/server/serve.mjs > "$WORK/serve.mjs"
sed "s/18088/${PORT}/g" bench/server/app.mjs > "$WORK/app.mjs"

teardown_port() {
	local pids
	for _ in $(seq 1 50); do
		pids=$(ss -lntpH "sport = :$PORT" 2>/dev/null |
			grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u || true)
		[[ -z $pids ]] && break
		# shellcheck disable=SC2086
		kill -9 $pids 2>/dev/null || true
		sleep 0.1
	done
	if ss -lntH "sport = :$PORT" 2>/dev/null | grep -q .; then
		echo "server-baseline.sh: port $PORT is still held; aborting" >&2
		exit 1
	fi
}

start_server() {
	local runtime="$1"
	local scenario="$2"
	local js="$WORK/app.mjs"
	local bin="$KILN_SERVER"

	case "$runtime" in
	node) exec node "$js" ;;
	bun) exec bun run "$js" ;;
	deno) exec deno run --allow-net --quiet "$js" ;;
	kiln) exec "$bin" ;;
	*) echo "Unknown runtime: $runtime" >&2; exit 2 ;;
	esac
}

# Determine tool versions and availability
NODE_VER="$(node --version 2>/dev/null || echo "")"
BUN_VER="$(bun --version 2>/dev/null || echo "")"
DENO_VER="$(deno --version 2>/dev/null | head -1 || echo "")"
KILN_VER="$("$KILN_BIN" --version 2>/dev/null | head -1 || echo "kiln 0.0.0")"

RESULTS_JSON="$WORK/raw_results.json"
echo "{}" > "$RESULTS_JSON"

echo "=== Running Server Scenarios Baseline ==="
echo "Clients: $CLIENTS, Duration per scenario: ${DURATION_MS}ms, Port: $PORT"
echo "Runtimes: node (${NODE_VER}), bun (${BUN_VER}), deno (${DENO_VER:-not on PATH}), kiln (${KILN_VER})"
echo ""

# Runtimes list
RUNTIMES=("node" "bun" "deno" "kiln")

for scenario in $SCENARIOS; do
	for runtime in "${RUNTIMES[@]}"; do
		# Check runtime availability
		if [[ "$runtime" == "deno" ]] && ! command -v deno >/dev/null 2>&1; then
			python3 - "$RESULTS_JSON" "$scenario" "$runtime" <<'PY'
import json, sys
res_file, scn, rt = sys.argv[1:4]
with open(res_file) as f: data = json.load(f)
if scn not in data: data[scn] = {}
data[scn][rt] = {"available": False, "reason": "deno is not installed on PATH"}
with open(res_file, "w") as f: json.dump(data, f)
PY
			continue
		fi
		if [[ "$runtime" == "bun" ]] && ! command -v bun >/dev/null 2>&1; then
			python3 - "$RESULTS_JSON" "$scenario" "$runtime" <<'PY'
import json, sys
res_file, scn, rt = sys.argv[1:4]
with open(res_file) as f: data = json.load(f)
if scn not in data: data[scn] = {}
data[scn][rt] = {"available": False, "reason": "bun is not installed on PATH"}
with open(res_file, "w") as f: json.dump(data, f)
PY
			continue
		fi

		teardown_port
		start_server "$runtime" "$scenario" >/dev/null 2>&1 &
		server_pid=$!

		# Wait for server to listen
		for _ in $(seq 1 50); do
			if curl -s -o /dev/null "http://127.0.0.1:$PORT/"; then break; fi
			sleep 0.1
		done

		if ! curl -s -o /dev/null "http://127.0.0.1:$PORT/"; then
			kill -9 "$server_pid" 2>/dev/null || true
			python3 - "$RESULTS_JSON" "$scenario" "$runtime" <<'PY'
import json, sys
res_file, scn, rt = sys.argv[1:4]
with open(res_file) as f: data = json.load(f)
if scn not in data: data[scn] = {}
data[scn][rt] = {"available": False, "reason": "server failed to answer on port"}
with open(res_file, "w") as f: json.dump(data, f)
PY
			continue
		fi

		# Drive scenario
		if [[ "$scenario" == "mixed" ]]; then
			out="$(node bench/server/drive-mix.mjs "$DURATION_MS" "$CLIENTS" "$PORT" 2>/dev/null || echo "{}")"
		else
			out="$(node bench/server/drive.mjs "$DURATION_MS" "$CLIENTS" "$PORT" "$scenario" 2>/dev/null || echo "{}")"
		fi

		# Teardown server
		kill "$server_pid" 2>/dev/null || true
		wait "$server_pid" 2>/dev/null || true
		teardown_port

		# Record metrics
		python3 - "$RESULTS_JSON" "$scenario" "$runtime" "$out" <<'PY'
import json, sys
res_file, scn, rt, out_str = sys.argv[1:5]
with open(res_file) as f: data = json.load(f)
if scn not in data: data[scn] = {}
try:
    parsed = json.loads(out_str)
    rps = parsed.get("rps", 0)
    p50 = parsed.get("p50_ms", 0.0)
    errs = parsed.get("errors", 0)
    data[scn][rt] = {"available": True, "rps": rps, "p50_ms": p50, "errors": errs}
except Exception as e:
    data[scn][rt] = {"available": False, "reason": f"invalid output: {e}"}
with open(res_file, "w") as f: json.dump(data, f)
PY
	done
done

# Analyze baseline comparison and render table
python3 - "$RESULTS_JSON" "$COMPARE" "$OUT" "$UPDATE" "$THRESHOLD" "$CLIENTS" "$DURATION_MS" "$NODE_VER" "$BUN_VER" "$DENO_VER" "$KILN_VER" <<'PY' | kiln_block
import json, os, sys

res_file, compare_file, out_file, update, threshold, clients, duration_ms = sys.argv[1:8]
node_ver, bun_ver, deno_ver, kiln_ver = sys.argv[8:12]
update = update == "1"
threshold = float(threshold)
clients = int(clients)

with open(res_file) as f:
    raw_results = json.load(f)

baseline = {}
if os.path.exists(compare_file) and not update:
    try:
        with open(compare_file) as f:
            baseline_doc = json.load(f)
            baseline = baseline_doc.get("scenarios", {})
    except Exception:
        baseline = {}

regressions = []
table_rows = []

for scn, targets in raw_results.items():
    node_res = targets.get("node", {})
    bun_res = targets.get("bun", {})
    deno_res = targets.get("deno", {})
    kiln_res = targets.get("kiln", {})

    node_str = f"{node_res['rps']} rps" if node_res.get("available") else "n/a"
    bun_str = f"{bun_res['rps']} rps" if bun_res.get("available") else "n/a"
    deno_str = f"{deno_res['rps']} rps" if deno_res.get("available") else "n/a"
    kiln_str = f"{kiln_res['rps']} rps" if kiln_res.get("available") else "fail"

    kiln_rps = kiln_res.get("rps", 0) if kiln_res.get("available") else 0
    base_entry = baseline.get(scn, {}).get("kiln", {})
    base_rps = base_entry.get("rps") if base_entry.get("available") else None

    if base_rps is not None and base_rps > 0:
        base_str = f"{base_rps} rps"
        delta = ((kiln_rps - base_rps) / base_rps) * 100.0
        delta_str = f"{delta:+.1f}%"
        # No-regression rule: check if kiln regressed more than threshold
        if delta < -threshold:
            status = "REGRESSION"
            regressions.append(f"{scn}: {base_rps} rps -> {kiln_rps} rps ({delta:.1f}%)")
        else:
            status = "pass"
    else:
        base_str = "-"
        delta_str = "-"
        status = "pass" if kiln_res.get("available") else "fail"

    table_rows.append({
        "scenario": scn,
        "clients": clients,
        "node": node_str,
        "bun": bun_str,
        "deno": deno_str,
        "kiln": kiln_str,
        "baseline": base_str,
        "delta": delta_str,
        "status": status,
    })

# Format table
header = f"{'scenario':<20} {'clients':>7} {'node':>12} {'bun':>12} {'deno':>10} {'kiln':>12} {'baseline':>12} {'delta':>8} {'status':>11}"
print(header)
print("-" * len(header))
for r in table_rows:
    print(f"{r['scenario']:<20} {r['clients']:>7} {r['node']:>12} {r['bun']:>12} {r['deno']:>10} {r['kiln']:>12} {r['baseline']:>12} {r['delta']:>8} {r['status']:>11}")

output_doc = {
    "schema": 2,
    "clients": clients,
    "duration_ms": int(duration_ms),
    "threshold_pct": threshold,
    "versions": {
        "node": node_ver or None,
        "bun": bun_ver or None,
        "deno": deno_ver or None,
        "kiln": kiln_ver or None,
    },
    "scenarios": raw_results,
}

if update or not os.path.exists(out_file):
    os.makedirs(os.path.dirname(os.path.abspath(out_file)), exist_ok=True)
    with open(out_file, "w") as f:
        json.dump(output_doc, f, indent=2)
    print(f"\nWrote baseline document to {out_file}")

if regressions:
    print("\n*** SERVER BASELINE REGRESSIONS DETECTED ***")
    for reg in regressions:
        print(f"  REGRESSION in scenario {reg}")
    sys.exit(1)
else:
    print(f"\nAll {len(table_rows)} scenarios passed no-regression check (threshold: {threshold}%).")
PY

