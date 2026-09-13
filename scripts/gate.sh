#!/usr/bin/env bash
#
# Everything that has to hold before a change lands, in one command.
#
# The pieces already existed as separate scripts; what did not exist was a single thing to run,
# so each agent assembled their own in a scratch directory and got it subtly wrong. This is that
# assembly, in the repository, where a fix to it is a fix for everyone.
#
# Ordered cheapest-first so a broken build fails in seconds rather than after the corpora:
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

QUICK=0
[[ ${1:-} == --quick ]] && QUICK=1

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
	local status=0
	"$@" >"$temporary" 2>&1 || status=$?
	if [[ -s $logfile ]]; then rm -f "$temporary"; else mv -f "$temporary" "$logfile"; fi
	echo "full log: $logfile"
	tail -20 "$logfile" || true
	if [[ $status -eq 0 ]]; then return 0; fi
	FAILURES+=("$name")
	return 0
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
kiln_cargo build --release --locked -p kiln-cli >"$BUILD_LOG" 2>&1 || {
	echo "full log: $BUILD_LOG" >&2
	tail -20 "$BUILD_LOG" >&2 || true
	echo "gate: release build failed" >&2
	exit 1
}
echo "full log: $BUILD_LOG"
KILN_BIN="$(kiln_repo_root)/target/release/kiln"
export KILN_BIN
KILN_TEST262_KILN="$KILN_BIN"
export KILN_TEST262_KILN
KILN_BENCH_KILN="$KILN_BIN"
export KILN_BENCH_KILN

# `--test-threads` is bounded because several of `kiln-native`'s tests each shell out to a C
# compiler, and this repository is routinely built by more than one agent at once: unbounded, they
# starve each other -- eleven failed here while all eighty-nine passed when run alone. A gate that
# flakes is worse than no gate, and `mbx` bounds compilation but not a test binary's thread pool.
#
# A broken *test* build is the one a release build does not notice: the binary is fine and the
# suite never ran. Asserting on the summary line rather than the exit code is what catches it.
# shellcheck disable=SC2016
step "crate tests" "$BASH" -c '
	test_status=0
	mbx test --workspace -- --test-threads=4 >"$GATE_STEP_LOG" 2>&1 || test_status=$?
	output="$(cat "$GATE_STEP_LOG")"
	# The failing *names* as well as the summary: a count says the suite is red and nothing
	# about which part, and this step is the one that reports breakage in any other crate.
	# The failure *bodies*, not just their names: a name says which test and nothing about
	# why, and these failures have been transient enough that a second occurrence may be
	# the only chance to read one.
	printf "%s\n" "$output" | grep -E -A6 "^---- .* stdout" | head -60
	printf "%s\n" "$output" | grep -E "^(test result: FAILED|error)" | head -10
	# One total rather than the last three summary lines. The last suites of a workspace run
	# are doc-test targets with nothing in them, so a tail of "0 passed; 0 failed" three times
	# reads as a suite that never ran -- two agents read it that way on 2026-09-06 and went
	# looking for a test cache that does not exist. A total says what happened, and the floor
	# below is the assertion the comment above promises: a build that produced no test binary
	# prints no summary lines at all and would otherwise pass on "no FAILED".
	printf "%s\n" "$output" | awk "/^test result:/ { suites++; passed += \$4; failed += \$6 } END { printf \"crate tests: %d suites, %d passed, %d failed\\n\", suites, passed, failed }"
	total_passed="$(printf "%s\n" "$output" | awk "/^test result:/ { passed += \$4 } END { print passed + 0 }")"
	if [[ $total_passed -lt ${KILN_GATE_MIN_PASSED:-500} ]]; then
		echo "crate tests: only $total_passed tests passed, below the floor of ${KILN_GATE_MIN_PASSED:-500}: the suite did not run" >&2
		exit 1
	fi
	[[ $test_status -eq 0 ]] && ! grep -qE "^error|FAILED" <<<"$output"
'

step "native parity" expect_clean "native" "0 diverged" "$BASH" ./scripts/native.sh
step "thirdparty" expect_clean "thirdparty" "0 diverged" "$BASH" ./scripts/thirdparty.sh

if [[ $QUICK -eq 0 ]]; then
	step "thirdparty baseline" ./scripts/thirdparty.sh --bench
	step "facts baseline" ./scripts/facts-baseline.sh --check
	for corpus in node server regress; do
		step "conformance/$corpus" "$BASH" -c "
			status=0
			\"\$KILN_BIN\" --one-shot test conformance/$corpus --node >\"\$GATE_STEP_LOG\" 2>&1 || status=\$?
			out=\"\$(cat \"\$GATE_STEP_LOG\")\"
			printf '%s\n' \"\$out\" | grep -E '^  (pass|fail) '
			[[ \$status -eq 0 && \$(grep -cE '^  fail +[1-9]' <<<\"\$out\") -eq 0 ]]
		"
	done
	step "test262 baseline" ./scripts/test262-baseline.sh
fi

printf '\n=== gate\n'
if [[ ${#FAILURES[@]} -gt 0 ]]; then
	printf 'FAILED: %s\n' "${FAILURES[*]}" >&2
	exit 1
fi
echo "all clear"
