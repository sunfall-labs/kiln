#!/usr/bin/env bash
# Every runtime, both workloads, several client processes, interleaved passes: the table a claim
# about throughput should come from. Servers are started on PORT one at a time.
#
# usage: bench/server/compare.sh <kiln-hello-binary> <kiln-app-binary>
# The kiln binaries must already listen on PORT: build them from copies of serve.mjs and app.mjs
# with 18088 replaced, e.g. `sed s/18088/$PORT/g bench/server/app.mjs > /tmp/app.mjs`.
# Environment: PASSES (3), PROCS (4), CONNS (16 per process), DURATION_MS (5000), PORT (18088),
# RUNTIMES ("kiln node bun deno").
set -euo pipefail
kiln_hello="${1:?path to the natively compiled serve.mjs}"
kiln_app="${2:?path to the natively compiled app.mjs}"
PASSES="${PASSES:-3}"; PROCS="${PROCS:-4}"; CONNS="${CONNS:-16}"; DURATION_MS="${DURATION_MS:-5000}"
PORT="${PORT:-18088}"; RUNTIMES="${RUNTIMES:-kiln node bun deno}"
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
sed "s/18088/${PORT}/g" "$HERE/serve.mjs" > "$work/serve.mjs"
sed "s/18088/${PORT}/g" "$HERE/app.mjs" > "$work/app.mjs"

start() { # runtime workload
	local js="$work/serve.mjs" bin="$kiln_hello"
	[[ $2 == mixed ]] && js="$work/app.mjs" && bin="$kiln_app"
	# exec so the background job *is* the server and killing $! kills it, not a wrapper shell.
	case "$1" in
	kiln) exec "$bin" ;;
	node) exec node "$js" ;;
	bun) exec bun run "$js" ;;
	deno) exec deno run --allow-net --quiet "$js" ;;
	esac
}

uptime | sed 's/.*load/load/'
for pass in $(seq 1 "$PASSES"); do
	for workload in hello mixed; do
		for runtime in $RUNTIMES; do
			if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
				echo "compare.sh: port $PORT is busy" >&2; exit 2
			fi
			start "$runtime" "$workload" >/dev/null 2>&1 &
			server=$!
			for _ in $(seq 1 50); do curl -s -o /dev/null "http://127.0.0.1:$PORT/" && break; sleep 0.1; done
			printf 'pass=%s workload=%-5s runtime=%-4s ' "$pass" "$workload" "$runtime"
			bash "$HERE/drive-multi.sh" "$PROCS" "$CONNS" "$DURATION_MS" "$PORT" "$workload"
			echo "  rss_kib=$(ps -o rss= -p "$server" | tr -d ' ')"
			kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true
			# A kiln server with KILN_WORKERS>1 forks; killing the parent reparents the children
			# to init, where they keep the SO_REUSEPORT listener open and quietly serve the *next*
			# run. That is not a slow teardown, it is a wrong number: measured once, twenty
			# orphans had accumulated and every reading after the first was the whole pool.
			for _ in $(seq 1 50); do
				# `|| true` because an already-free port makes `grep` exit non-zero, and under
				# `set -e` a failing command substitution in an assignment ends the run.
				pids=$(ss -lntpH "sport = :$PORT" 2>/dev/null |
					grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u || true)
				[[ -z $pids ]] && break
				# shellcheck disable=SC2086
				kill -9 $pids 2>/dev/null || true
				sleep 0.1
			done
			if ss -lntH "sport = :$PORT" 2>/dev/null | grep -q .; then
				printf 'port %s is still held after teardown; refusing to report a contaminated run\n' "$PORT" >&2
				exit 1
			fi
			sleep 0.3
		done
	done
done
uptime | sed 's/.*load/load/'
rm -rf "$work"
