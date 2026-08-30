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
# ONE TRAP. node sleeps for real and kiln's clock is virtual, so a probe that schedules something
# an hour out finishes instantly here and hangs the reference implementation there. Keep timer
# delays to a few milliseconds and assert *ordering*; the fact that the delay's magnitude is free
# is covered by unit tests, where there is no node to wait for. See ADR 0012.
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
	cargo build --locked -p kiln-cli >&2
	KILN="$(kiln_repo_root)/target/debug/kiln"
fi

matched=0
diverged=0
declare -a REFUSALS=()
declare -a DIVERGENCES=()

for probe in "$PROBES"/*.js; do
	name="$(basename -- "$probe" .js)"
	expected="$(node "$probe" 2>&1 || true)"
	# --one-shot so a stale daemon can never answer for a build that no longer exists.
	actual="$("$KILN" --one-shot run "$probe" 2>&1 || true)"

	if [[ $actual == "$expected" ]]; then
		matched=$((matched + 1))
		continue
	fi
	if [[ $actual == kiln:* ]]; then
		# The first line carries the reason; the path prefix in it is noise here.
		REFUSALS+=("$name	$(printf '%s' "$actual" | head -1 | sed 's/^kiln: [^:]*: //')")
		continue
	fi
	diverged=$((diverged + 1))
	DIVERGENCES+=("$name")
	DIVERGENCES+=("  node: $(printf '%s' "$expected" | head -5)")
	DIVERGENCES+=("  kiln: $(printf '%s' "$actual" | head -5)")
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
