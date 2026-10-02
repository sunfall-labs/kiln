#!/usr/bin/env bash
#
# Everything that has to hold before a change lands, in one command.
#
# The pieces already existed as separate scripts; what did not exist was a single thing to run,
# so each agent assembled their own in a scratch directory and got it subtly wrong. This is that
# assembly, in the repository, where a fix to it is a fix for everyone.
#
# Build once, then overlap independent correctness lanes within one CPU/memory budget.
# Thirdparty benchmark binaries may compile alongside those lanes; their warmups and timed runs
# wait until every correctness lane has joined, so tests cannot perturb the measurements.
# The lanes cover:
#
#   1. crate tests          -- seconds; catches a broken build, which the filtered output of a
#                              release build otherwise hides behind a working binary
#   2. native parity        -- the compiled backend against node, probe by probe
#   3. thirdparty bundles   -- real libraries, both engines
#   4. the three corpora    -- node, server, regress
#   5. Test262 baseline     -- chunk by chunk against the committed numbers
#
# `--quick` stops after (3), which is the loop worth running while iterating.
#
# Both `native.sh` and `thirdparty.sh` need a bash with `BASHPID`; macOS ships 3.2 as `/bin/bash`,
# and under it `thirdparty.sh`'s cleanup trap deletes the shared temp directory instead of its own.
# They are executed rather than passed to `bash` so their `#!/usr/bin/env bash` picks the newer one.
set -euo pipefail

# macOS ships bash 3.2 as `/bin/bash`, and `#!/usr/bin/env bash` finds it whenever a caller's PATH
# does not put a newer one first -- which `mise run` does not. Under 3.2 `thirdparty.sh` loses
# `wait -n` and `BASHPID`, and its cleanup trap deletes the *shared* results directory instead of
# its own, so every bundle reports "missing" and the gate blames the change under test. Re-exec
# once under a bash that has them rather than leaving it to the caller's environment.
if [[ ${BASH_VERSINFO[0]} -lt 4 ]]; then
	for candidate in /opt/homebrew/bin/bash /usr/local/bin/bash "$(command -v bash)"; do
		# shellcheck disable=SC2016
		if [[ -x $candidate ]] && [[ $("$candidate" -c 'echo ${BASH_VERSINFO[0]}') -ge 4 ]]; then
			exec "$candidate" "${BASH_SOURCE[0]}" "$@"
		fi
	done
	echo "gate: needs bash 4 or newer; install one (brew install bash)" >&2
	exit 1
fi

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"
export KILN_CODE_SNAPSHOT_SHA256="$(python3 -c 'import sys; sys.path.insert(0, "scripts"); import gate_results; print(gate_results.code_snapshot())')"

QUICK=0
export GATE_TEST262_SUITE=routine
case ${1:-} in
    --quick) QUICK=1 ;;
    --full) GATE_TEST262_SUITE=full ;;
    "") ;;
    *) echo "usage: scripts/gate.sh [--quick|--full]" >&2; exit 2 ;;
esac
printf 'Test262 suite: %s (gate-full includes extended coverage)\n' "$GATE_TEST262_SUITE"
GATE_STARTED=$SECONDS
budget="$(python3 scripts/gate_budget.py "$@")"
read -r GATE_PARALLEL GATE_JOBS TEST_JOBS NATIVE_JOBS TEST262_JOBS TEST262_MEMORY_MB <<<"$budget"
export KILN_JOBS="$NATIVE_JOBS" RAYON_NUM_THREADS="$NATIVE_JOBS"
THIRDPARTY_PREP_JOBS=0
if [[ $QUICK -eq 0 && $GATE_PARALLEL -eq 1 && $GATE_JOBS -ge 4 ]]; then
	THIRDPARTY_PREP_JOBS=1
fi
printf 'gate budget: %s CPUs; crate tests %s, native %s, Test262 %s (%s MiB cap), thirdparty prepare %s\n' \
	"$GATE_JOBS" "$TEST_JOBS" "$NATIVE_JOBS" "$TEST262_JOBS" "$TEST262_MEMORY_MB" "$THIRDPARTY_PREP_JOBS"
BACKGROUND_PIDS=()
THIRDPARTY_PID=""
THIRDPARTY_STAGE_DIR=""
THIRDPARTY_RELEASE_MARKER=""

cleanup_thirdparty_stage() {
	if [[ -n $THIRDPARTY_PID ]]; then
		if [[ -n $THIRDPARTY_RELEASE_MARKER && ! -e $THIRDPARTY_RELEASE_MARKER ]]; then
			printf 'abort\n' >"$THIRDPARTY_RELEASE_MARKER.tmp"
			mv -f "$THIRDPARTY_RELEASE_MARKER.tmp" "$THIRDPARTY_RELEASE_MARKER"
		fi
		wait "$THIRDPARTY_PID" 2>/dev/null || true
		THIRDPARTY_PID=""
	fi
	if [[ -n $THIRDPARTY_STAGE_DIR ]]; then
		rm -rf "$THIRDPARTY_STAGE_DIR"
		THIRDPARTY_STAGE_DIR=""
	fi
}
trap cleanup_thirdparty_stage EXIT

record_elapsed() {
    python3 - "$1" "$2" <<'PYTIME'
import json, sys
from pathlib import Path
path = Path(sys.argv[1])
doc = json.loads(path.read_text())
doc["elapsed_seconds"] = int(sys.argv[2])
path.write_text(json.dumps(doc, indent=2) + "\n")
PYTIME
}

FAILURES=()
GATE_LOG_ROOT="${GATE_LOG_DIR:-$PWD/target/gate-logs}"
mkdir -p "$GATE_LOG_ROOT"
GATE_LOG_DIR="$(mktemp -d "$GATE_LOG_ROOT/run.XXXXXXXX")"
export GATE_LOG_DIR
GATE_STEP_NUMBER=0
# Every step runs even when an earlier one failed, and the failures are reported together at the
# end: which *combination* broke is the useful signal, and stopping at the first hides it. `set -e`
# is still on -- an `if` condition is exempt from it, which is what makes this safe rather than a
# reason to turn it off.
step() {
	local name="$1"
	shift
	local safe_name="${name//[^A-Za-z0-9_.-]/_}"
	GATE_STEP_NUMBER=$((GATE_STEP_NUMBER + 1))
	local logfile="$GATE_LOG_DIR/$(printf '%02d-%s.log' "$GATE_STEP_NUMBER" "$safe_name")"
	local temporary="$logfile.tmp"
	export GATE_STEP_LOG="$logfile"
	printf '\n=== %s\n' "$name"
	local started=$SECONDS
	local status=0
	"$@" >"$temporary" 2>&1 || status=$?
	local elapsed=$((SECONDS - started))
	if [[ $name == "crate tests" ]]; then
		elapsed=$((elapsed + GATE_CACHE_TEST_SECONDS))
	fi
	if [[ -s $logfile ]]; then
		# The command wrote the raw stream directly to the final log.  Preserve the filtered
		# aggregate too; the gate's crate total and failure names are part of the evidence.
		{
			printf '\n--- gate summary ---\n'
			cat "$temporary"
		} >>"$logfile"
		rm -f "$temporary"
	else
		mv -f "$temporary" "$logfile"
	fi
	echo "full log: $logfile"
	tail -20 "$logfile" || true
	local envelope_status=0
	if [[ -f "$logfile.json" ]]; then
		cp "$logfile.json" "$logfile.envelope.json"
	else
		python3 "$PWD/scripts/gate_results.py" process --name "$name" --log "$logfile" --status "$status" --out "$logfile.envelope.json" || envelope_status=$?
	fi
	if [[ $envelope_status -ne 0 ]]; then
		status=$envelope_status
	fi
	record_elapsed "$logfile.envelope.json" "$elapsed"
	printf '%s: %ss\n' "$name" "$elapsed"
	if [[ $status -eq 0 ]]; then return 0; fi
	FAILURES+=("$name")
	return 0
}

background_step() {
    if [[ $GATE_PARALLEL -eq 0 ]]; then
        step "$@"
    else
        # The child reserves the next number inside step; advance the parent's counter too.
        step "$@" &
        BACKGROUND_PIDS+=("$!")
        GATE_STEP_NUMBER=$((GATE_STEP_NUMBER + 1))
    fi
}

wait_background() {
    local pid
    for pid in "${BACKGROUND_PIDS[@]}"; do
        wait "$pid" || FAILURES+=("background lane (pid $pid)")
    done
    BACKGROUND_PIDS=()
}

expect_clean() {
	local name="$1" pattern="$2"
	shift 2
	local status=0
	"$@" >"$GATE_STEP_LOG" 2>&1 || status=$?
	grep -qE "$pattern" "$GATE_STEP_LOG" && [[ $status -eq 0 ]]
}

printf '=== build\n'
BUILD_LOG="$GATE_LOG_DIR/00-build.log"
BUILD_STARTED=$SECONDS
kiln_cargo build --release --locked -p kiln-cli >"$BUILD_LOG" 2>&1 || {
	echo "full log: $BUILD_LOG" >&2
	tail -20 "$BUILD_LOG" >&2 || true
	python3 "$PWD/scripts/gate_results.py" process --name build --log "$BUILD_LOG" --status 1 --out "$GATE_LOG_DIR/00-build.envelope.json" || true
	python3 "$PWD/scripts/gate_results.py" aggregate --directory "$GATE_LOG_DIR" --out "$GATE_LOG_DIR/aggregate.json" \
		--expected build --expected "crate tests" --expected "native parity" --expected thirdparty \
		--expected "thirdparty baseline" --expected "facts baseline" --expected "conformance/node" \
		--expected "conformance/server" --expected "conformance/regress" --expected "test262 baseline" || true
	echo "gate: release build failed" >&2
	exit 1
}
echo "full log: $BUILD_LOG"
KILN_BIN="$(kiln_repo_root)/target/release/kiln"
export KILN_BIN
python3 "$PWD/scripts/gate_results.py" process --name build --log "$BUILD_LOG" --status 0 --out "$GATE_LOG_DIR/00-build.envelope.json"
record_elapsed "$GATE_LOG_DIR/00-build.envelope.json" "$((SECONDS - BUILD_STARTED))"
KILN_TEST262_KILN="$KILN_BIN"
export KILN_TEST262_KILN
KILN_BENCH_KILN="$KILN_BIN"
export KILN_BENCH_KILN

# Keep durable cache publication/sweep tests away from concurrent native compiler writes.
# On the measured filesystem the full suite took 45s alongside them and under one second alone.
# Its raw summaries join the workspace evidence below, and its status cannot be discarded.
printf '\n=== artifact cache tests (before concurrent filesystem work)\n'
export GATE_CACHE_TEST_LOG="$GATE_LOG_DIR/artifact-cache.raw"
export GATE_CACHE_TEST_STATUS=0
CACHE_TEST_STARTED=$SECONDS
mbx test -p kiln-artifact-cache --locked --jobs="$TEST_JOBS" -- --test-threads="$TEST_JOBS" >"$GATE_CACHE_TEST_LOG" 2>&1 || GATE_CACHE_TEST_STATUS=$?
GATE_CACHE_TEST_SECONDS=$((SECONDS - CACHE_TEST_STARTED))

# Compile native-release benchmark binaries while the correctness lanes use the rest of the
# gate budget. The benchmark script blocks at the release marker after compilation; only after
# all correctness work joins does the gate release it to warm and time binaries serially.
THIRDPARTY_BENCH_ARGS=(--bench)
if [[ $GATE_TEST262_SUITE == full ]]; then
	THIRDPARTY_BENCH_ARGS+=(--include-postcss)
fi
THIRDPARTY_STAGED=0
if [[ $THIRDPARTY_PREP_JOBS -eq 1 ]]; then
	THIRDPARTY_STAGE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/kiln-gate-thirdparty.XXXXXXXX")"
	THIRDPARTY_RELEASE_MARKER="$THIRDPARTY_STAGE_DIR/release"
	step "thirdparty baseline" env KILN_THIRDPARTY_WORK_TMPDIR="$THIRDPARTY_STAGE_DIR" \
		KILN_THIRDPARTY_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1 \
		./scripts/thirdparty.sh "${THIRDPARTY_BENCH_ARGS[@]}" --wait-for-release "$THIRDPARTY_RELEASE_MARKER" &
	THIRDPARTY_PID=$!
	GATE_STEP_NUMBER=$((GATE_STEP_NUMBER + 1))
	THIRDPARTY_STAGED=1
fi

# `--test-threads` is bounded because several of `kiln-native`'s tests each shell out to a C
# compiler, and this repository is routinely built by more than one agent at once: unbounded, they
# starve each other -- eleven failed here while all eighty-nine passed when run alone. A gate that
# flakes is worse than no gate, and `mbx` bounds compilation but not a test binary's thread pool.
#
# A broken *test* build is the one a release build does not notice: the binary is fine and the
# suite never ran. Asserting on the summary line rather than the exit code is what catches it.
# shellcheck disable=SC2016
background_step "crate tests" env KILN_JOBS="$TEST_JOBS" RAYON_NUM_THREADS="$TEST_JOBS" KILN_TEST_THREADS="$TEST_JOBS" "$BASH" -c '
	test_status="$GATE_CACHE_TEST_STATUS"
	raw_log="${GATE_STEP_LOG}.raw"
	cat "$GATE_CACHE_TEST_LOG" >"$raw_log"
	mbx test --workspace --exclude kiln-artifact-cache --locked --no-fail-fast --jobs="$KILN_TEST_THREADS" -- --test-threads="$KILN_TEST_THREADS" >>"$raw_log" 2>&1 || test_status=$?
	output="$(cat "$raw_log")"
	# The failing *names* as well as the summary: a count says the suite is red and nothing
	# about which part, and this step is the one that reports breakage in any other crate.
	# The failure *bodies*, not just their names: a name says which test and nothing about
	# why, and these failures have been transient enough that a second occurrence may be
	# the only chance to read one.
	printf "%s\n" "$output" | grep -E -A6 "^---- .* stdout" | head -60 || true
	printf "%s\n" "$output" | grep -E "^(test result: FAILED|error)" | head -10 || true
	# One total rather than the last three summary lines. The last suites of a workspace run
	# are doc-test targets with nothing in them, so a tail of "0 passed; 0 failed" three times
	# reads as a suite that never ran -- two agents read it that way on 2026-09-06 and went
	# looking for a test cache that does not exist. A total says what happened, and the floor
	# below is the assertion the comment above promises: a build that produced no test binary
	# prints no summary lines at all and would otherwise pass on "no FAILED".
	parser_status=0
	python3 "$PWD/scripts/gate_results.py" workspace --name "crate tests" --log "$raw_log" --status "$test_status" --out "${GATE_STEP_LOG}.json" || parser_status=$?
	[[ $test_status -eq 0 && $parser_status -eq 0 ]]
'

if [[ $QUICK -eq 0 ]]; then
    background_step "test262 baseline" env KILN_JOBS="$TEST262_JOBS" RAYON_NUM_THREADS="$TEST262_JOBS" KILN_TEST262_MEMORY_MB="$TEST262_MEMORY_MB" "$BASH" -c '
		status=0
		raw_log="${GATE_STEP_LOG}.raw"
		result="${GATE_STEP_LOG}.results.json"
		KILN_TEST262_REPORT="$result" ./scripts/test262-baseline.sh --suite "$GATE_TEST262_SUITE" >"$raw_log" 2>&1 || status=$?
		cat "$raw_log"
		parser_status=0
		python3 "$PWD/scripts/gate_results.py" test262 --name "test262 baseline" --log "$raw_log" --status "$status" --document "$result" --out "${GATE_STEP_LOG}.json" || parser_status=$?
		[[ $status -eq 0 && $parser_status -eq 0 ]]
	'
fi

step "native parity" expect_clean "native" "0 diverged" ./scripts/native.sh
step "thirdparty" expect_clean "thirdparty" "0 diverged" ./scripts/thirdparty.sh

if [[ $QUICK -eq 0 ]]; then
	for corpus in node server regress; do
		step "conformance/$corpus" "$BASH" -c "
			status=0
			raw_log=\"\${GATE_STEP_LOG}.raw\"
			result=\"\${GATE_STEP_LOG}.results.json\"
			\"\$KILN_BIN\" --one-shot test conformance/$corpus --node --out \"\$result\" >\"\$raw_log\" 2>&1 || status=\$?
			out=\"\$(cat \"\$raw_log\")\"
			printf '%s\n' \"\$out\" | grep -E '^  (pass|fail) '
			parser_status=0
			python3 \"\$PWD/scripts/gate_results.py\" conformance --name \"conformance/$corpus\" --log \"\$raw_log\" --status \"\$status\" --document \"\$result\" --out \"\${GATE_STEP_LOG}.json\" || parser_status=\$?
			[[ \$status -eq 0 && \$parser_status -eq 0 ]]
		"
	done

fi

wait_background
# Timings are meaningful only after the CPU-heavy correctness lanes have joined.
if [[ $QUICK -eq 0 ]]; then
	if [[ $THIRDPARTY_STAGED -eq 1 ]]; then
		printf 'measure\n' >"$THIRDPARTY_RELEASE_MARKER.tmp"
		mv -f "$THIRDPARTY_RELEASE_MARKER.tmp" "$THIRDPARTY_RELEASE_MARKER"
		# The background `step` stores command status in its envelope; aggregate validates that
		# required envelope and verdict even though the step helper itself returns normally.
		wait "$THIRDPARTY_PID" || FAILURES+=("thirdparty baseline (pid $THIRDPARTY_PID)")
		THIRDPARTY_PID=""
		rm -rf "$THIRDPARTY_STAGE_DIR"
		THIRDPARTY_STAGE_DIR=""
		THIRDPARTY_RELEASE_MARKER=""
	else
		# Serial gates and smaller budgets keep preparation after correctness and use the released
		# CPU budget. Nested kiln builds remain single-worker; binary timing stays serial.
		step "thirdparty baseline" env KILN_THIRDPARTY_BUILD_JOBS="$GATE_JOBS" KILN_JOBS=1 RAYON_NUM_THREADS=1 ./scripts/thirdparty.sh "${THIRDPARTY_BENCH_ARGS[@]}"
	fi
    step "facts baseline" ./scripts/facts-baseline.sh --check
fi

printf '\n=== gate\n'
printf 'completed in %ss\n' "$((SECONDS - GATE_STARTED))"
if [[ $QUICK -eq 1 ]]; then
	EXPECTED_STEPS=(build "crate tests" "native parity" thirdparty)
else
	EXPECTED_STEPS=(build "crate tests" "native parity" thirdparty "thirdparty baseline" "facts baseline" "conformance/node" "conformance/server" "conformance/regress" "test262 baseline")
fi
aggregate_args=()
for expected in "${EXPECTED_STEPS[@]}"; do aggregate_args+=(--expected "$expected"); done
python3 "$PWD/scripts/gate_results.py" aggregate --directory "$GATE_LOG_DIR" --out "$GATE_LOG_DIR/aggregate.json" "${aggregate_args[@]}" || FAILURES+=("gate aggregate")
if [[ ${#FAILURES[@]} -gt 0 ]]; then
	printf 'FAILED: %s\n' "${FAILURES[*]}" >&2
	exit 1
fi
printf 'all clear (%ss)\n' "$((SECONDS - GATE_STARTED))"
