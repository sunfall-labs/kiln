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
#   match     both engines printed the same bytes.
#   refused   the backend said which instruction it does not implement yet. Not a failure: the
#             native surface is deliberately growing, and each refusal names itself, so the list
#             below is a work queue rather than a defect report.
#   DIVERGES  both ran to completion and printed different bytes. That is a compiler bug.
#
# A probe that the *interpreter* cannot run is a failure either way: this corpus is not the place
# to discover that, and a probe nobody can run measures nothing.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

PROBES="${KILN_NATIVE_DIR:-conformance/native}"

KILN="${KILN_BIN:-}"
if [[ -z $KILN ]]; then
	kiln_cargo build --locked -p kiln-cli >&2
	KILN="$(kiln_repo_root)/target/debug/kiln"
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
	if [[ $name == *.cjs ]]; then
		adapter="--node"
	fi
	name="${name%.*}"

	local interpreted
	if ! interpreted="$("$KILN" --one-shot ${adapter:+"$adapter"} run "$probe" 2>&1)"; then
		printf '%s\n' "$interpreted" > "$WORK_DIR/$name.failed"
		return 0
	fi

	local native
	native="$("$KILN" --one-shot ${adapter:+"$adapter"} run --native "$probe" 2>&1 || true)"

	if [[ $native == "$interpreted" ]]; then
		touch "$WORK_DIR/$name.matched"
		return 0
	fi

	local refusal
	refusal="$(printf '%s\n' "$native" | grep -m1 '^kiln: native ' || true)"
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
		printf '%s\n' "  interpreter: $(printf '%s' "$interpreted" | head -5)"
		printf '%s\n' "  native:      $(printf '%s' "$native" | head -5)"
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
for probe in "$PROBES"/*.ts "$PROBES"/*.js "$PROBES"/*.cjs; do
	run_probe "$probe" &
	running=$((running + 1))
	if (( running >= JOBS )); then
		wait -n
		running=$((running - 1))
	fi
done
wait

for probe in "$PROBES"/*.ts "$PROBES"/*.js "$PROBES"/*.cjs; do
	name="$(basename -- "$probe")"
	name="${name%.*}"
	if [[ -f "$WORK_DIR/$name.failed" ]]; then
		kiln_say "FAILED: the interpreter cannot run $name, so there is nothing to compare:"
		kiln_say "$(<"$WORK_DIR/$name.failed")"
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

kiln_say "native: $matched matched, ${#REFUSALS[@]} refused, $diverged diverged"

if ((${#REFUSALS[@]} > 0)); then
	kiln_say ""
	kiln_say "Refused by the native backend (the backlog, not a failure):"
	printf '%s\n' "${REFUSALS[@]}" | sort -t'	' -k2 | while IFS=$'\t' read -r name reason; do
		kiln_say "$(printf '  %-28s %s' "$name" "$reason")"
	done
fi

if ((diverged > 0)); then
	kiln_say ""
	kiln_say "DIVERGED - the two engines ran the same bytecode and printed different bytes:"
	printf '%s\n' "${DIVERGENCES[@]}" | while IFS= read -r line; do kiln_say "$line"; done
	kiln_say ""
	kiln_say "One of the two is wrong. The bytecode is the same, so nothing else can explain it."
	exit 1
fi
