#!/usr/bin/env bash
#
# Run conformance/native through both execution engines and fail on any disagreement.
#
# WHY THE INTERPRETER IS THE ORACLE. Everywhere else in this repository the reference is node,
# because the question is whether kiln agrees with JavaScript. Here the question is narrower and
# the answer is sharper: the interpreter and the native backend consume the *same bytecode*, so
# if they print different bytes one of them is wrong and there is nothing else it could be. No
# specification reading, no node version, no locale enters into it.
#
# THE THREE OUTCOMES, and only one is a failure:
#
#   match     both engines printed the same bytes *and* exited with the same status.
#   refused   the backend said which instruction it does not implement yet. Not a failure: the
#             native surface is deliberately growing, and each refusal names itself, so the list
#             below is a work queue rather than a defect report.
#   DIVERGES  both ran to completion and disagreed -- on the bytes, or on the exit status. That
#             is a compiler bug.
#
# THE EXIT STATUS IS PART OF THE ANSWER. This compared output alone for a long time, and a
# successful CommonJS program that printed the right bytes and then exited 1 was a *match* to it
# for weeks -- while anything running that program from a shell, a Makefile or CI saw a failure.
# A program is a pair of things: what it wrote and what it returned. Comparing half of that is
# how the half nobody compares goes wrong.
#
# A probe that the *interpreter* cannot run is a failure either way: this corpus is not the place
# to discover that, and a probe nobody can run measures nothing.
# `wait -n` and `BASHPID` need bash 4.3 or newer. macOS ships 3.2 as /bin/bash, and under it this
# script's cleanup trap deletes the shared results directory instead of its own while the script
# still exits 0 -- two full corpus runs were lost to that on 2026-09-05. Re-exec once under a bash
# that has them, the way scripts/gate.sh does, rather than leaving it to the caller's environment.
if [[ ${BASH_VERSINFO[0]} -lt 4 ]]; then
	for candidate in /opt/homebrew/bin/bash /usr/local/bin/bash "$(command -v bash)"; do
		# shellcheck disable=SC2016
		if [[ -x $candidate ]] && [[ $("$candidate" -c 'echo ${BASH_VERSINFO[0]}') -ge 4 ]]; then
			exec "$candidate" "${BASH_SOURCE[0]}" "$@"
		fi
	done
	echo "native.sh: needs bash 4 or newer; install one (brew install bash)" >&2
	exit 1
fi

set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

PROBES="${KILN_NATIVE_DIR:-conformance/native}"

# Run the *native* side of each pair under the collector's stress mode.
#
# The ordinary corpus collects when the heap says to, which for most probes is never, so a value
# the walk failed to root is swept only if a collection happens to land inside the window that
# drops it. `conformance/native/json-shaped-build-edges.js` nests 200 deep and passed for as long
# as it existed while `JSON.parse` was handing back one key's bytes under another key's name --
# the collection never landed. Under stress it lands on every allocation and the same probes ask
# a much sharper question: is every value this program builds reachable at every point it could
# be collected?
#
# The interpreter stays ordinary. It is the oracle here (see above), and an oracle is worth
# having only if it is the cheap, boring one.
#
# It is minutes rather than seconds, so it is a task of its own (`mise run native-stress`) and
# not part of `gate-quick`.
STRESS="${KILN_NATIVE_GC_STRESS:-}"

KILN="${KILN_BIN:-}"
if [[ -z $KILN ]]; then
	if [[ -x "$(kiln_repo_root)/target/release/kiln" ]]; then
		KILN="$(kiln_repo_root)/target/release/kiln"
	else
		kiln_cargo build --locked -p kiln-cli >&2
		KILN="$(kiln_repo_root)/target/debug/kiln"
	fi
fi

matched=0
diverged=0
declare -a REFUSALS=()
declare -a DIVERGENCES=()

JOBS="${KILN_JOBS:-$(nproc 2>/dev/null || echo 4)}"
if (( JOBS > 16 )); then
	JOBS=16
elif (( JOBS < 1 )); then
	JOBS=1
fi

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/kiln-native-results.XXXXXXXX")"
cleanup() {
	if [[ ${BASHPID:-$$} -eq $$ ]]; then
		rm -rf "$WORK_DIR"
	fi
}
trap cleanup EXIT

run_probe() {
	trap - EXIT
	local probe="$1"
	local name
	name="$(basename -- "$probe")"
	local adapter=""
	# `.mjs` too: an application-shaped ES module imports `node:` builtins, and only the adapter
	# resolves those. Two such probes sat here unrun from 2026-09-19 until the glob below grew.
	if [[ $name == *.cjs || $name == *.mjs ]]; then
		adapter="--node"
	fi
	name="${name%.*}"

	# A probe may ask for an environment on its first line, `// native-env: NAME=value ...`, and
	# both engines then run under it -- the one way to hold a run-time switch such as
	# `KILN_EXTENSIONS=0` (ADR 0068) to the same comparison as everything else. The whole line is
	# the environment; nothing else in a probe is read.
	local -a probe_env=()
	local directive
	directive="$(sed -n '1s|^// native-env: ||p' "$probe")"
	if [[ -n $directive ]]; then
		read -r -a probe_env <<<"$directive"
	fi

	local interpreted
	if ! interpreted="$(env ${probe_env[@]+"${probe_env[@]}"} "$KILN" --one-shot ${adapter:+"$adapter"} run "$probe" 2>&1)"; then
		printf '%s\n' "$interpreted" > "$WORK_DIR/$name.failed"
		return 0
	fi
	# Zero by construction: a nonzero interpreter took the branch above and is already a failure.
	# Named anyway, because the comparison below is between the two engines and reads as one.
	local interpreted_status=0

	local native native_status=0
	if [[ -n $STRESS ]]; then
		native="$(env ${probe_env[@]+"${probe_env[@]}"} KILN_GC_STRESS="$STRESS" "$KILN" --one-shot ${adapter:+"$adapter"} run --native "$probe" 2>&1)" || native_status=$?
	else
		native="$(env ${probe_env[@]+"${probe_env[@]}"} "$KILN" --one-shot ${adapter:+"$adapter"} run --native "$probe" 2>&1)" || native_status=$?
	fi

	if [[ $native == "$interpreted" ]] && ((native_status == interpreted_status)); then
		touch "$WORK_DIR/$name.matched"
		return 0
	fi

	# A build that failed is not a backend that refused.
	#
	# `^kiln: native ` matched both, so `native toolchain failed:` and `native number provider
	# failed:` -- a broken link, a compiler whose target disagrees with the provider's -- were
	# counted under "the backlog, not a failure" and the run still reported success. That is how
	# this corpus answered `0 matched, 89 refused` and `69 matched, 20 refused` on consecutive
	# runs of the same tree without anything saying a word: every one of those refusals was a
	# build that did not happen.
	local broken
	broken="$(printf '%s\n' "$native" | grep -m1 -E '^kiln: native (toolchain|number provider) failed' || true)"
	if [[ -n $broken ]]; then
		printf '%s\n' "$native" > "$WORK_DIR/$name.broken"
		return 0
	fi

	# A real refusal names the backlog it hit: the backend declining a construct it cannot emit
	# yet. Both spellings are listed rather than prefix-matched, so a new `kiln: native ...`
	# message has to be classified by somebody instead of quietly joining the backlog.
	local refusal
	refusal="$(printf '%s\n' "$native" |
		grep -m1 -E '^kiln: native (backend refusal:|emission failed: unsupported native emission:)' ||
		true)"
	if [[ -n $refusal ]]; then
		local reason
		reason="$(printf '%s' "$refusal" | sed 's/^kiln: //; s/ module=[^ ]*//')"
		if [[ $native != kiln:* ]]; then
			reason="$reason (after $(printf '%s\n' "$native" | grep -c -v '^kiln: native ') line(s) of output)"
		fi
		printf '%s\n' "$reason" > "$WORK_DIR/$name.refused"
		return 0
	fi

	{
		printf '%s\n' "  interpreter: $(printf '%s' "$interpreted" | head -5) [exit $interpreted_status]"
		printf '%s\n' "  native:      $(printf '%s' "$native" | head -5) [exit $native_status]"
		if [[ $native == "$interpreted" ]]; then
			printf '%s\n' "  the bytes agree; only the exit status differs"
		fi
	} > "$WORK_DIR/$name.diverged"
}

# A `.cjs` probe is here for one reason: it is the only way sloppy code reaches either engine.
# `kiln run` compiles a `.ts` or `.js` entry as ESM, which is strict by definition, so every probe
# in this corpus was strict and the whole sloppy half of the language was invisible to the one
# gate that compares the two engines. That is how `OrdinaryCallBindThis` came to be implemented in
# the interpreter alone: the corpus could not have told anyone. The Node Adapter is what classifies
# a `.cjs` entry as CommonJS, so those probes get `--node` -- on *both* sides, so the comparison
# stays what it says it is.
shopt -s nullglob
running=0
for probe in "$PROBES"/*.ts "$PROBES"/*.js "$PROBES"/*.cjs "$PROBES"/*.mjs; do
	run_probe "$probe" &
	running=$((running + 1))
	if (( running >= JOBS )); then
		wait -n || true
		running=$((running - 1))
	fi
done
wait || true

for probe in "$PROBES"/*.ts "$PROBES"/*.js "$PROBES"/*.cjs "$PROBES"/*.mjs; do
	name="$(basename -- "$probe")"
	name="${name%.*}"
	if [[ -f "$WORK_DIR/$name.failed" ]]; then
		kiln_say "FAILED: the interpreter cannot run $name, so there is nothing to compare:"
		kiln_say "$(<"$WORK_DIR/$name.failed")"
		exit 1
	elif [[ -f "$WORK_DIR/$name.broken" ]]; then
		kiln_say "FAILED: the native build of $name did not complete, so there is nothing to compare:"
		kiln_say "$(<"$WORK_DIR/$name.broken")"
		exit 1
	elif [[ -f "$WORK_DIR/$name.matched" ]]; then
		matched=$((matched + 1))
	elif [[ -f "$WORK_DIR/$name.refused" ]]; then
		REFUSALS+=("$name	$(<"$WORK_DIR/$name.refused")")
	elif [[ -f "$WORK_DIR/$name.diverged" ]]; then
		diverged=$((diverged + 1))
		DIVERGENCES+=("$name")
		while IFS= read -r line; do
			DIVERGENCES+=("$line")
		done < "$WORK_DIR/$name.diverged"
	fi
done

kiln_say "native: $matched matched, ${#REFUSALS[@]} refused, $diverged diverged${STRESS:+ (KILN_GC_STRESS=$STRESS)}"

if ((${#REFUSALS[@]} > 0)); then
	kiln_say ""
	kiln_say "Refused by the native backend (the backlog, not a failure):"
	printf '%s\n' "${REFUSALS[@]}" | sort -t'	' -k2 | while IFS=$'\t' read -r name reason; do
		kiln_say "$(printf '  %-28s %s' "$name" "$reason")"
	done
fi

if ((diverged > 0)); then
	kiln_say ""
	kiln_say "DIVERGED - the two engines ran the same bytecode and did not agree:"
	printf '%s\n' "${DIVERGENCES[@]}" | while IFS= read -r line; do kiln_say "$line"; done
	kiln_say ""
	kiln_say "One of the two is wrong. The bytecode is the same, so nothing else can explain it."
	exit 1
fi
