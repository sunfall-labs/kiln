#!/usr/bin/env bash
#
# Diff kiln against node over conformance/differential, and fail on any answer that is wrong
# without saying so.
#
# WHY THIS EXISTS ALONGSIDE THE CONFORMANCE CORPUS. conformance/manifest.toml records what each
# probe *should* print, so it catches a known behaviour breaking. It cannot catch a behaviour
# nobody thought to record, and that is where the dangerous bugs have been: a parameter default
# silently dropped, `slice` clamping a negative bound to zero, `JSON.parse` returning undefined.
# Every one of those passed the corpus, because the corpus had nothing to say about them.
#
# So this compares against a live `node` instead of a recorded expectation. The corpus answers
# "did anything break?"; this answers "what is still wrong?" - and the second question is the one
# with no fixed answer, which is why the outcome here is a classification rather than a score.
#
# THE THREE OUTCOMES, and only one of them is a failure:
#
#   match     kiln and node printed the same bytes.
#   refused   kiln printed a diagnostic and stopped. This is the *expected* state for most of the
#             corpus and is not a failure: the language subset is deliberately growing, and a
#             refusal is a backlog entry that names itself. They are listed, grouped by reason,
#             for exactly that reason.
#   DIVERGES  kiln ran to completion and printed something else. This is a silent wrong answer,
#             it fails this script, and it is the whole point of the file.
#
# Probes are ordinary JavaScript that prints to stdout and exits 0 under node. Add one whenever
# you want to know what kiln does with a piece of the language; it costs nothing to be refused.
#
# TWO TRAPS, both about the clock.
#
# node sleeps for real and kiln's clock is virtual, so a probe that schedules something an hour
# out finishes instantly here and hangs the reference implementation there. Keep timer delays to a
# few milliseconds and assert *ordering*; the fact that the delay's magnitude is free is covered by
# unit tests, where there is no node to wait for. See ADR 0012.
#
# And kiln's realm is always UTC, deliberately -- a timezone is host state, and a program that
# reads it is not reproducible. node takes its zone from the environment, so it is run under
# TZ=UTC below to make the two comparable. A probe must therefore not print `Date.now()` or an
# unqualified `new Date()`: those are the *current* instant, which is real time there and virtual
# time here and can never agree. See ADR 0015.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

PROBES="${KILN_DIFFERENTIAL_DIR:-conformance/differential}"

if ! command -v node >/dev/null 2>&1; then
	# Skipped rather than failed: the reference implementation is the one dependency this check
	# cannot fake, and pretending to have run it would be worse than saying it did not.
	kiln_say "SKIP: node is not on PATH, so there is nothing to compare against."
	exit 0
fi

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

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/kiln-diff-results.XXXXXXXX")"
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
	name="$(basename -- "$probe" .js)"

	local expected
	expected="$(TZ=UTC node "$probe" 2>&1 || true)"

	local actual
	local kiln_status=0
	actual="$("$KILN" --one-shot run "$probe" 2>&1)" || kiln_status=$?

	if [[ $actual == "$expected" ]]; then
		touch "$WORK_DIR/$name.matched"
		return 0
	fi

	if ((kiln_status != 0)) && [[ $actual == kiln:* || $actual == *$'\n'kiln:* ]]; then
		local reason
		reason="$(printf '%s\n' "$actual" | sed -n '/^kiln:/ { s/^kiln: [^:]*: //; p; q; }')"
		printf '%s\n' "$reason" > "$WORK_DIR/$name.refused"
		return 0
	fi

	{
		printf '%s\n' "  node: $(printf '%s' "$expected" | head -5)"
		printf '%s\n' "  kiln: $(printf '%s' "$actual" | head -5)"
	} > "$WORK_DIR/$name.diverged"
}

running=0
for probe in "$PROBES"/*.js; do
	run_probe "$probe" &
	running=$((running + 1))
	if (( running >= JOBS )); then
		wait -n
		running=$((running - 1))
	fi
done
wait

for probe in "$PROBES"/*.js; do
	name="$(basename -- "$probe" .js)"
	if [[ -f "$WORK_DIR/$name.matched" ]]; then
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

kiln_say "differential: $matched matched, ${#REFUSALS[@]} refused, $diverged diverged"

if ((${#REFUSALS[@]} > 0)); then
	kiln_say ""
	kiln_say "Refused (the backlog, not a failure):"
	printf '%s\n' "${REFUSALS[@]}" | sort -t'	' -k2 | while IFS=$'\t' read -r name reason; do
		kiln_say "$(printf '  %-28s %s' "$name" "$reason")"
	done
fi

if ((diverged > 0)); then
	kiln_say ""
	kiln_say "DIVERGED - kiln ran and printed something other than node did:"
	printf '%s\n' "${DIVERGENCES[@]}" | while IFS= read -r line; do kiln_say "$line"; done
	kiln_say ""
	kiln_say "A wrong answer with no diagnostic is the one outcome this check exists to stop."
	kiln_say "Either fix the behaviour, or refuse the construct until it is implemented."
	exit 1
fi
