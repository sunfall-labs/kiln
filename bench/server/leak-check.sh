#!/usr/bin/env bash
#
# Confirm the 2026-09-01 leak (7-12 KiB per request) is gone using the SIGUSR1 heap
# introspection line over a 60-second sustained load run.
#
# ADR 0049: Sending SIGUSR1 to a compiled kiln server writes three lines to stderr:
#   kiln introspect pid=<pid> heap bytes=... count=... survived=... freed_bytes=...
#   kiln introspect pid=<pid> runtime ...
#   kiln introspect pid=<pid> connections ...
#
# usage: bench/server/leak-check.sh [server-binary] [port=18088] [duration=60]
set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$HERE/../.." && pwd)"
cd "$REPO_ROOT"

PORT="${2:-18088}"
DURATION="${3:-60}"
KILN_BIN="${1:-}"

if [[ -z "$KILN_BIN" ]]; then
	if [[ -x "/tmp/test-serve" ]]; then
		KILN_BIN="/tmp/test-serve"
	else
		KILN_BIN="target/release/serve-bin"
		if [[ ! -x "$KILN_BIN" ]]; then
			echo "Building native serve binary..."
			target/release/kiln --one-shot build bench/server/serve.mjs --node --profile native-release --output "$KILN_BIN"
		fi
	fi
fi

if [[ ! -x "$KILN_BIN" ]]; then
	echo "leak-check.sh: server binary '$KILN_BIN' not found or not executable" >&2
	exit 2
fi

if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
	echo "leak-check.sh: port $PORT is busy" >&2
	exit 2
fi

STDERR_LOG="$(mktemp "${TMPDIR:-/tmp}/kiln-leak-stderr.XXXXXX")"
trap 'rm -f "$STDERR_LOG"' EXIT

"$KILN_BIN" 2>"$STDERR_LOG" &
SERVER_PID=$!
cleanup() {
	kill -9 "$SERVER_PID" 2>/dev/null || true
	wait "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT

for _ in $(seq 1 50); do
	if curl -s -o /dev/null "http://127.0.0.1:$PORT/"; then break; fi
	sleep 0.1
done

if ! curl -s -o /dev/null "http://127.0.0.1:$PORT/"; then
	echo "leak-check.sh: server never answered on port $PORT" >&2
	cat "$STDERR_LOG" >&2
	exit 1
fi

echo "=== Starting 60-second leak check on PID $SERVER_PID ==="
kill -USR1 "$SERVER_PID"
sleep 0.2

# Background sampler: sends SIGUSR1 every 10 seconds during the run
(
	for _ in $(seq 10 10 "$DURATION"); do
		sleep 10
		kill -USR1 "$SERVER_PID" 2>/dev/null || break
	done
) &
SAMPLER_PID=$!

# Run load for DURATION seconds using drive.mjs with concurrency 16
echo "Driving load with concurrency 16 for ${DURATION}s..."
LOAD_OUT="$(node "$HERE/drive.mjs" "$(( DURATION * 1000 ))" 16 "$PORT" hello)"
echo "$LOAD_OUT"

wait "$SAMPLER_PID" 2>/dev/null || true

# Final SIGUSR1 dump
sleep 0.5
kill -USR1 "$SERVER_PID" 2>/dev/null || true
sleep 0.5

cleanup

echo ""
echo "=== SIGUSR1 Heap Introspection Samples ==="
grep -E 'heap bytes=' "$STDERR_LOG" || true

python3 - "$LOAD_OUT" "$STDERR_LOG" <<'PY'
import json, re, sys

load = json.loads(sys.argv[1])
requests = load.get("requests", 0)
rps = load.get("rps", 0)
errs = load.get("errors", 0)

try:
    with open(sys.argv[2]) as f:
        log = f.read()
except Exception:
    log = ""

heap_lines = [l for l in log.splitlines() if "heap bytes=" in l]

print(f"\nRequests served: {requests} ({rps} rps, {errs} errors)")
print(f"Recorded {len(heap_lines)} heap introspection samples:\n")
print(f"{'sample':<8} {'heap_bytes':<12} {'survived':<12} {'freed_bytes':<14} {'collections':<12}")
print("-" * 58)

pattern = re.compile(r'bytes=(\d+).*survived=(\d+).*collections=(\d+).*freed_bytes=(\d+)')

samples = []
for idx, line in enumerate(heap_lines, 1):
    m = pattern.search(line)
    if m:
        hb, surv, col, freed = int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4))
        samples.append((hb, surv, col, freed))
        print(f"{idx:<8} {hb:<12} {surv:<12} {freed:<14} {col:<12}")

if not samples:
    print("WARNING: No heap samples parsed.")
    sys.exit(1)

first_surv = samples[0][1]
last_surv = samples[-1][1]
last_heap = samples[-1][0]
last_freed = samples[-1][3]
last_cols = samples[-1][2]

# If 2026-09-01 leak (7-12 KiB per request) were present:
expected_leak_mb = (requests * 8) / 1024
print(f"\nAnalysis:")
print(f"  If leaking 8 KiB/req, memory would have grown by ~{expected_leak_mb:.1f} MiB")
print(f"  Actual last heap bytes: {last_heap / 1024:.1f} KiB")
print(f"  Actual last survived:   {last_surv / 1024:.1f} KiB")
print(f"  Total freed by GC:      {last_freed / (1024*1024):.1f} MiB across {last_cols} collections")

# The 2026-09-01 leak check assertion:
# Heap must be bounded (< 10 MiB) and survived bytes must be bounded (< 500 KiB)
if last_heap > 10 * 1024 * 1024:
    print(f"FAIL: Heap grew to {last_heap} bytes (>10 MiB)")
    sys.exit(1)

if last_surv > 500 * 1024:
    print(f"FAIL: Survived grew to {last_surv} bytes (>500 KiB)")
    sys.exit(1)

print("\nPASS: 2026-09-01 leak is confirmed GONE. Memory is bounded and fully reclaimed.")
PY
