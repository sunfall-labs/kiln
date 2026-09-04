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
# Every step runs even when an earlier one failed, and the failures are reported together at the
# end: which *combination* broke is the useful signal, and stopping at the first hides it. `set -e`
# is still on -- an `if` condition is exempt from it, which is what makes this safe rather than a
# reason to turn it off.
step() {
	local name="$1"
	shift
	printf '\n=== %s\n' "$name"
	if "$@"; then
		return 0
	fi
	FAILURES+=("$name")
	return 0
}

expect_clean() {
	local name="$1" pattern="$2"
	shift 2
	local output=""
	# `|| true` because a non-zero exit is one of the things being reported, not a reason to
	# abandon the gate: the summary line below says what actually happened.
	output="$("$@" 2>&1)" || true
	printf '%s\n' "$output" | tail -20
	grep -qE "$pattern" <<<"$output"
}

printf '=== build\n'
kiln_cargo build --release --locked -p kiln-cli >/dev/null || {
	echo "gate: release build failed" >&2
	exit 1
}

# `--test-threads` is bounded because several of `kiln-native`'s tests each shell out to a C
# compiler, and this repository is routinely built by more than one agent at once: unbounded, they
# starve each other -- eleven failed here while all eighty-nine passed when run alone. A gate that
# flakes is worse than no gate, and `mbx` bounds compilation but not a test binary's thread pool.
#
# A broken *test* build is the one a release build does not notice: the binary is fine and the
# suite never ran. Asserting on the summary line rather than the exit code is what catches it.
step "crate tests" "$BASH" -c '
	output="$(mbx test --workspace -- --test-threads=4 2>&1)"
	printf "%s\n" "$output" | grep -E "^(test result|error)" | tail -12
	! grep -qE "^error|FAILED" <<<"$output"
'

step "native parity" expect_clean "native" "0 diverged" "$BASH" ./scripts/native.sh
step "thirdparty" expect_clean "thirdparty" "0 diverged" "$BASH" ./scripts/thirdparty.sh

if [[ $QUICK -eq 0 ]]; then
	for corpus in node server regress; do
		step "conformance/$corpus" "$BASH" -c "
			out=\"\$(./target/release/kiln --one-shot test conformance/$corpus --node 2>&1)\"
			printf '%s\n' \"\$out\" | grep -E '^  (pass|fail) '
			[[ \$(grep -cE '^  fail +[1-9]' <<<\"\$out\") -eq 0 ]]
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
