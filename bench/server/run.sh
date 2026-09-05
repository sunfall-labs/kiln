#!/usr/bin/env bash
#
# Throughput and resident size of the ten-line HTTP server, one runtime at a time.
#
# `drive.mjs` measures requests per second and latency; this wraps it with the two things a
# single drive cannot see: the server's resident size before and after the load (a collector
# that is not collecting shows up here, not in rps) and repetition, because a single 5 s run on a
# shared laptop drifts by several hundred rps. Rounds are interleaved across concurrencies so a
# background build that starts mid-run hurts every row rather than one.
#
# usage: bench/server/run.sh <label> <command...>
#   bench/server/run.sh kiln /tmp/serve-kiln
#   bench/server/run.sh node node bench/server/serve.mjs
#   bench/server/run.sh bun  bun  bench/server/serve.mjs
#
# Environment: DURATION_MS (default 5000), ROUNDS (default 3), CONCURRENCIES (default "1 16 64"),
# WORKLOAD (default "hello": drive.mjs against serve.mjs; "mixed": drive-mix.mjs against app.mjs),
# PORT (default 18088; the server you pass must listen there).
# Never gates. The numbers are properties of this machine on this afternoon.
set -euo pipefail

label="${1:?usage: run.sh <label> <command...>}"
shift
if [[ $# -eq 0 ]]; then
	echo "run.sh: a server command is required after the label" >&2
	exit 2
fi

DURATION_MS="${DURATION_MS:-5000}"
ROUNDS="${ROUNDS:-3}"
CONCURRENCIES="${CONCURRENCIES:-1 16 64}"
WORKLOAD="${WORKLOAD:-hello}"
PORT="${PORT:-18088}"
case "$WORKLOAD" in
mixed) DRIVER=drive-mix.mjs ;;
*) DRIVER=drive.mjs ;;
esac
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
	echo "run.sh: something already listens on $PORT; stop it first" >&2
	exit 2
fi

"$@" >/dev/null 2>"/tmp/kiln-bench-$label.err" &
server=$!
trap 'kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true' EXIT

for _ in $(seq 1 50); do
	if curl -s -o /dev/null "http://127.0.0.1:$PORT/"; then break; fi
	sleep 0.1
done
if ! curl -s -o /dev/null "http://127.0.0.1:$PORT/"; then
	echo "run.sh: $label never answered on $PORT" >&2
	cat "/tmp/kiln-bench-$label.err" >&2
	exit 1
fi

rss() { ps -o rss= -p "$server" | tr -d ' '; }

echo "server=$label workload=$WORKLOAD pid=$server rss_start_kib=$(rss)"
for round in $(seq 1 "$ROUNDS"); do
	for c in $CONCURRENCIES; do
		printf 'round=%s c=%-3s ' "$round" "$c"
		if [[ "$WORKLOAD" == "mixed" ]]; then
			node "$HERE/$DRIVER" "$DURATION_MS" "$c" "$PORT" | cut -c1-160
		else
			node "$HERE/$DRIVER" "$DURATION_MS" "$c" "$PORT" "$WORKLOAD" | cut -c1-160
		fi
	done
done
echo "server=$label rss_end_kib=$(rss) alive=$(ps -p "$server" -o pid= | tr -d ' ')"
if [[ -s "/tmp/kiln-bench-$label.err" ]]; then
	echo "--- stderr from $label:"
	tail -5 "/tmp/kiln-bench-$label.err"
fi
