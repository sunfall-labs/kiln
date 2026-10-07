#!/usr/bin/env bash
#
# End-to-end check of process-mode supervision (issue 07): a KILN_WORKERS=4 server
#   (a) serves from every worker, (e) each seeing KILN_WORKER_INDEX 0..3,
#   (b) restarts a worker killed with SIGKILL and gets its capacity back,
#   (c) drains every worker and exits 143 on SIGTERM to the supervisor,
#   (d) loses every worker when the supervisor itself is SIGKILLed (Linux: PR_SET_PDEATHSIG),
#   (f) puts each worker in its own process group, so a terminal's Ctrl-C (SIGINT to the
#       supervisor's group) reaches the workers once, through the supervisor, and exits 130,
#   (g) SIGKILLs a worker that does not stop within KILN_WORKERS_GRACE_MS (a SIGSTOPped worker
#       stands in for one that ignores SIGTERM: a stopped process never acts on it),
#   (h) treats a worker's exit 1 as the program's answer: no restart, every worker stopped, the
#       supervisor exits 1,
#   (i) with a binary built `--workers=auto` and KILN_WORKERS unset, runs one worker per core.
#
# usage: supervise-e2e.sh <binary of serve-index.mjs> <the same, built --workers=auto>
#   target/release/kiln --one-shot build docs/issues/realms-and-concurrency/probes/serve-index.mjs \
#       --node --profile native-release --output /some/dir/serve-index
#   ... the same with --workers=auto --output /some/dir/serve-index-auto
set -uo pipefail

BIN="${1:?usage: supervise-e2e.sh <binary> <binary built --workers=auto>}"
AUTO_BIN="${2:?usage: supervise-e2e.sh <binary> <binary built --workers=auto>}"
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
# `setsid` gives the supervisor a session and process group of its own, as a shell's job would,
# so (f) can signal that group without signalling this script.
start() { # binary [env assignment...]
	local bin=$1
	shift
	env "$@" setsid "$bin" >/dev/null 2>>"$LOG" &
	sup=$!
	wait_port || { fail "server never answered"; cat "$LOG"; exit 1; }
	sleep 0.3 # let every worker reach listen
}
stop_quietly() { # the supervisor we started and its workers, by PID only
	local kids
	kids="$(children "$sup")"
	kill -9 "$sup" $kids 2>/dev/null
	wait "$sup" 2>/dev/null
}

if curl -s -o /dev/null --max-time 1 "http://127.0.0.1:$PORT/"; then
	echo "something already listens on $PORT" >&2
	exit 2
fi

start "$BIN" KILN_WORKERS=$WORKERS
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

start "$BIN" KILN_WORKERS=$WORKERS
kids="$(children "$sup")"
kill -9 "$sup"
wait "$sup" 2>/dev/null
if wait_gone $kids; then pass "(d) supervisor SIGKILLed: every worker exited"; else fail "(d) orphaned workers: $kids"; kill -9 $kids 2>/dev/null; fi
curl -s -o /dev/null --max-time 1 "http://127.0.0.1:$PORT/" && fail "(d) port still answers" || pass "(d) port closed"

start "$BIN" KILN_WORKERS=$WORKERS
kids="$(children "$sup")"
groups_ok=1
for k in $kids; do
	[[ "$(ps -o pgid= -p "$k" | tr -d ' ')" == "$k" ]] || groups_ok=0
done
[[ "$(ps -o pgid= -p "$sup" | tr -d ' ')" == "$sup" ]] || groups_ok=0
[[ $groups_ok == 1 ]] && pass "(f) each worker leads its own process group, apart from the supervisor's" || fail "(f) pgids: $(ps -o pid=,pgid= -p "$sup" $kids | tr '\n' ';')"
lines_before=$(wc -l <"$LOG")
kill -INT -- "-$sup" # what a terminal does on Ctrl-C: the whole foreground group
wait "$sup"
rc=$?
if wait_gone $kids; then pass "(f) SIGINT to the supervisor's group: exited $rc, every worker gone"; else fail "(f) workers left: $kids"; fi
[[ $rc == 130 ]] && pass "(f) exit status 130 (128+SIGINT)" || fail "(f) exit status $rc"
[[ $(wc -l <"$LOG") == "$lines_before" ]] && pass "(f) nothing restarted or reported" || fail "(f) stderr: $(tail -n +$((lines_before + 1)) "$LOG")"

start "$BIN" KILN_WORKERS=$WORKERS KILN_WORKERS_GRACE_MS=1000
kids="$(children "$sup")"
stuck="$(echo "$kids" | tail -1)"
kill -STOP "$stuck"
t0=$(date +%s%N)
kill -TERM "$sup"
wait "$sup"
rc=$?
ms=$((($(date +%s%N) - t0) / 1000000))
if wait_gone $kids; then pass "(g) stuck worker: supervisor exited $rc after ${ms} ms, every worker gone"; else fail "(g) workers left: $kids"; kill -9 $kids 2>/dev/null; fi
[[ $rc == 143 && $ms -ge 1000 && $ms -lt 3000 ]] && pass "(g) status 143 after the 1000 ms grace, not before" || fail "(g) status $rc after $ms ms"
grep -q "(pid $stuck) did not stop within 1000 ms; sending SIGKILL" "$LOG" && pass "(g) escalation logged: $(grep 'sending SIGKILL' "$LOG" | tail -1)" || fail "(g) no escalation line in stderr"

start "$BIN" KILN_WORKERS=$WORKERS PROBE_EXIT_WORKER=2
kids="$(children "$sup")"
t0=$(date +%s%N)
wait "$sup"
rc=$?
ms=$((($(date +%s%N) - t0) / 1000000))
if wait_gone $kids; then pass "(h) worker 2 exited 1: supervisor stopped by itself after ${ms} ms, every worker gone"; else fail "(h) workers left: $kids"; kill -9 $kids 2>/dev/null; fi
[[ $rc == 1 ]] && pass "(h) supervisor exit status 1, the worker's" || fail "(h) exit status $rc"
grep -q "worker 2 (pid [0-9]*) exited with status 1; stopping the other workers" "$LOG" && pass "(h) logged: $(grep 'stopping the other' "$LOG" | tail -1)" || fail "(h) no stop line in stderr"
grep -q "exited with status 1.*restarting" "$LOG" && fail "(h) a deliberate exit was restarted" || pass "(h) not restarted"

start "$AUTO_BIN" -u KILN_WORKERS
kids="$(children "$sup")"
cores=$(nproc)
[[ $(echo "$kids" | wc -l) == "$cores" ]] && pass "(i) --workers=auto, KILN_WORKERS unset: $cores workers on $cores cores" || fail "(i) workers: $(echo "$kids" | wc -l), cores $cores"
stop_quietly
start "$AUTO_BIN" KILN_WORKERS=2
kids="$(children "$sup")"
[[ $(echo "$kids" | wc -l) == 2 ]] && pass "(i) KILN_WORKERS=2 overrides the recorded auto" || fail "(i) workers with KILN_WORKERS=2: $(echo "$kids" | wc -l)"
stop_quietly

echo "--- supervisor stderr"
cat "$LOG"
echo "failures: $failures"
exit $((failures != 0))
