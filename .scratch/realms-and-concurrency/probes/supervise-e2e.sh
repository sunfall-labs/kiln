#!/usr/bin/env bash
#
# End-to-end check of process-mode supervision (issue 07): a KILN_WORKERS=4 server
#   (a) serves from every worker, (e) each seeing KILN_WORKER_INDEX 0..3,
#   (b) restarts a worker killed with SIGKILL and gets its capacity back,
#   (c) drains every worker and exits 143 on SIGTERM to the supervisor,
#   (d) loses every worker when the supervisor itself is SIGKILLed (Linux: PR_SET_PDEATHSIG).
#
# usage: supervise-e2e.sh <native binary of serve-index.mjs>
#   target/release/kiln --one-shot build .scratch/realms-and-concurrency/probes/serve-index.mjs \
#       --node --profile native-release --output /some/dir/serve-index
set -uo pipefail

BIN="${1:?usage: supervise-e2e.sh <binary>}"
PORT=18088
WORKERS=4
LOG="$(mktemp -d)/stderr.log"
failures=0
pass() { echo "PASS $*"; }
fail() { echo "FAIL $*"; failures=$((failures + 1)); }

children() { pgrep -P "$1" | sort; }
wait_port() {
	for _ in $(seq 1 100); do
		curl -s -o /dev/null --max-time 1 "http://127.0.0.1:$PORT/" && return 0
		sleep 0.05
	done
	return 1
}
# Distinct worker indices seen over many fresh connections (SO_REUSEPORT hashes each one).
seen_indices() {
	for _ in $(seq 1 300); do curl -s --max-time 1 "http://127.0.0.1:$PORT/"; echo; done |
		grep -o 'worker [0-9]*/[0-9]*' | sort -u | tr '\n' ' '
}
wait_gone() { # pid... ; up to 5 s
	for _ in $(seq 1 100); do
		local alive=0
		for p in "$@"; do kill -0 "$p" 2>/dev/null && alive=1; done
		[[ $alive == 0 ]] && return 0
		sleep 0.05
	done
	return 1
}
start() {
	KILN_WORKERS=$WORKERS "$BIN" >/dev/null 2>>"$LOG" &
	sup=$!
	wait_port || { fail "server never answered"; cat "$LOG"; exit 1; }
	sleep 0.3 # let every worker reach listen
}

if curl -s -o /dev/null --max-time 1 "http://127.0.0.1:$PORT/"; then
	echo "something already listens on $PORT" >&2
	exit 2
fi

start
kids="$(children "$sup")"
echo "supervisor $sup, workers: $(echo $kids)"
[[ $(echo "$kids" | wc -l) == "$WORKERS" ]] && pass "(a) $WORKERS worker processes" || fail "(a) workers: $(echo $kids)"
seen="$(seen_indices)"
echo "seen: $seen"
[[ "$seen" == "worker 0/4 worker 1/4 worker 2/4 worker 3/4 " ]] &&
	pass "(a)+(e) every worker served; KILN_WORKER_INDEX 0..3, KILN_WORKER_COUNT 4" ||
	fail "(a)+(e) seen: $seen"

victim="$(echo "$kids" | head -1)"
kill -9 "$victim"
sleep 1
after="$(children "$sup")"
echo "after kill -9 $victim: $(echo $after)"
if [[ $(echo "$after" | wc -l) == "$WORKERS" ]] && ! echo "$after" | grep -qx "$victim"; then
	pass "(b) worker $victim replaced; $WORKERS workers again"
else
	fail "(b) workers after kill: $(echo $after)"
fi
seen="$(seen_indices)"
[[ "$seen" == "worker 0/4 worker 1/4 worker 2/4 worker 3/4 " ]] && pass "(b) capacity back: all four indices serve ($seen)" || fail "(b) seen: $seen"
grep -q "was killed by signal 9 .*restarting" "$LOG" && pass "(b) restart logged: $(grep 'restarting' "$LOG" | tail -1)" || fail "(b) no restart line in stderr"

kids="$(children "$sup")"
t0=$(date +%s%N)
kill -TERM "$sup"
wait "$sup"
rc=$?
ms=$((($(date +%s%N) - t0) / 1000000))
if wait_gone $kids; then pass "(c) SIGTERM: supervisor exited $rc after ${ms} ms, every worker gone"; else fail "(c) workers left: $kids"; fi
[[ $rc == 143 ]] && pass "(c) exit status 143 (128+SIGTERM)" || fail "(c) exit status $rc"
curl -s -o /dev/null --max-time 1 "http://127.0.0.1:$PORT/" && fail "(c) port still answers" || pass "(c) port closed"

start
kids="$(children "$sup")"
kill -9 "$sup"
wait "$sup" 2>/dev/null
if wait_gone $kids; then pass "(d) supervisor SIGKILLed: every worker exited"; else fail "(d) orphaned workers: $kids"; pkill -9 -f "$BIN"; fi
curl -s -o /dev/null --max-time 1 "http://127.0.0.1:$PORT/" && fail "(d) port still answers" || pass "(d) port closed"

echo "--- supervisor stderr"
cat "$LOG"
echo "failures: $failures"
exit $((failures != 0))
