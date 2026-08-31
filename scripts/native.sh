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

shopt -s nullglob
for probe in "$PROBES"/*.ts "$PROBES"/*.js; do
	name="$(basename -- "$probe")"
	name="${name%.*}"
	# --one-shot so a stale daemon can never answer for a build that no longer exists.
	if ! interpreted="$("$KILN" --one-shot run "$probe" 2>&1)"; then
		kiln_say "FAILED: the interpreter cannot run $name, so there is nothing to compare:"
		kiln_say "$interpreted"
		exit 1
	fi
	native="$("$KILN" --one-shot run --native "$probe" 2>&1 || true)"

	if [[ $native == "$interpreted" ]]; then
		matched=$((matched + 1))
		continue
	fi
	# A refusal is a refusal wherever it appears. It used to have to be on the *first* line,
	# which quietly turned every probe that printed something before reaching an unimplemented
	# operation into a reported divergence -- `conformance/native/temporal.ts` says in its own
	# header that this is the trap, and then fell into it anyway when the backend stopped
	# carrying one of the members it exercises. A divergence is the two engines disagreeing
	# about an answer; a backend that stops and says which operation it lacks has not given one.
	refusal="$(printf '%s\n' "$native" | grep -m1 '^kiln: native ' || true)"
	if [[ -n $refusal ]]; then
		reason="$(printf '%s' "$refusal" | sed 's/^kiln: //; s/ module=[^ ]*//')"
		# Whether anything ran first is worth saying: a probe refused on its first operation is
		# a missing feature, and one refused half way through is a missing feature plus a probe
		# that is no longer measuring what it was written to measure.
		if [[ $native != kiln:* ]]; then
			reason="$reason (after $(printf '%s\n' "$native" | grep -c -v '^kiln: native ') line(s) of output)"
		fi
		REFUSALS+=("$name	$reason")
		continue
	fi
	diverged=$((diverged + 1))
	DIVERGENCES+=("$name")
	DIVERGENCES+=("  interpreter: $(printf '%s' "$interpreted" | head -5)")
	DIVERGENCES+=("  native:      $(printf '%s' "$native" | head -5)")
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
