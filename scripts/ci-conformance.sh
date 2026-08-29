#!/usr/bin/env bash
#
# Compare the JavaScript conformance corpus against its recorded baseline, and fail the job if a
# test that used to pass no longer does.
#
# THE ASSUMED RUNNER INTERFACE. The runner that executes conformance/ is being built separately
# from this gate, so nothing here depends on its output format. What it does depend on - and what
# the two halves have to be reconciled against - is exactly this:
#
#   command      $KILN_CONFORMANCE_RUNNER when set; otherwise conformance/run.sh, if that file
#                exists and is executable. Neither present means "no runner yet", which is a
#                recorded state rather than a breakage; see below.
#   invocation   <command> --out <path>
#   contract     when it returns, a results document in the schema below is readable at <path>.
#   exit status  deliberately ignored. A conformance runner exits non-zero when tests fail, and
#                tests failing is the normal state of this corpus for a long time yet. What is
#                not ignored is the absence of a readable document: that fails this job, loudly,
#                because a runner that wrote nothing and a corpus that scored zero must never be
#                allowed to look alike.
#
# THE RESULTS SCHEMA, version 1. This half of the contract is defined here rather than derived
# from whatever the runner happens to print, so that the runner can change its human output
# without changing what is gated on:
#
#   {
#     "schema": 1,
#     "recorded": "2026-08-29",                     optional; echoed into the report
#     "runner": "kiln-cli 0.1.0",                   optional; echoed into the report
#     "totals": { "total": 32, "passed": 0, "failed": 0,
#                 "unsupported": 0, "skipped": 0, "not_yet_runnable": 32 },      optional
#     "tests": [ { "id": "guard/g01-spread-symbols", "outcome": "not-yet-runnable" }, ... ]
#   }
#
# `tests` is the load-bearing half and is required. `id` is the test id from
# conformance/manifest.toml, and the run must cover every one of them. `outcome` is one of pass,
# fail, unsupported, skipped, not-yet-runnable - kiln_proto::TestOutcome, plus the manifest's own
# status for a test no runner has reached. Extra keys on a test entry (a diagnostic, a duration)
# are allowed and ignored here, so the runner can record more than the gate reads. `totals` is
# optional but must agree with `tests` when present. kiln_conformance_results_digest in
# scripts/lib.sh is the validator, and it is the same one conformance/status.sh reads through.
#
# WHAT FAILS THE JOB: a test that passed at the baseline and does not pass now, named. Not a drop
# in the total - a total is not a regression check. "7,595 -> 7,651 passes" hides a test that
# started failing behind two that started passing, which is a lesson conformance/README.md
# carries over from the corpus's source project, along with the sharper one that a partial run
# must never be compared against a full baseline, still less overwrite one. More passes than the
# baseline never fails; it prints how to re-record the baseline, because the entire point of this
# number is that it climbs.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

MANIFEST="conformance/manifest.toml"
BASELINE="${KILN_CONFORMANCE_BASELINE:-conformance/results/baseline.json}"
# A run's own document goes under target/, which .gitignore already covers. Only the baseline is
# a committed number, and the cheapest way to keep it the only one is to make the transient one
# uncommittable by construction.
RESULTS="${KILN_CONFORMANCE_RESULTS:-target/conformance/results.json}"
DEFAULT_RUNNER="conformance/run.sh"

UPDATE=0
for arg in "$@"; do
	case "$arg" in
	--update-baseline)
		UPDATE=1
		;;
	-h | --help)
		cat <<-'USAGE'
			usage: scripts/ci-conformance.sh [--update-baseline]

			  (no flags)         Run the corpus and compare it against the recorded baseline.
			                     A test that passed at the baseline and does not pass now fails;
			                     passing more tests than the baseline never does.
			  --update-baseline  Record this run as the new baseline. A deliberate act, never done
			                     by CI: the baseline is committed so that moving it is a diff a
			                     reviewer has to accept.

			environment:
			  KILN_CONFORMANCE_RUNNER    runner command; see the contract at the top of this file
			  KILN_CONFORMANCE_BASELINE  baseline to compare against
			  KILN_CONFORMANCE_RESULTS   where this run's results document is written
		USAGE
		exit 0
		;;
	*)
		echo "ci-conformance.sh: unknown argument '$arg' (try --help)" >&2
		exit 2
		;;
	esac
done

# Everything that leaves the gate unable to compare exits through here, so that the job summary
# always names which half was missing instead of leaving a bare non-zero status behind.
fail() {
	kiln_say "### Conformance: the gate could not run"
	kiln_say ""
	for line in "$@"; do
		kiln_say "$line"
	done
	exit 1
}

# Validate a results document and leave its digest in DIGEST.
#
# The result comes back through a global rather than through stdout because a `fail` inside a
# command substitution would exit the subshell and let the job carry on with an empty digest -
# which is the silent zero this whole script exists to make impossible.
DIGEST=""
read_digest() {
	local label="$1" path="$2" output status=0
	output="$(kiln_conformance_results_digest "$path" 2>&1)" || status=$?
	if [[ $status -ne 0 ]]; then
		fail \
			"The $label results could not be read, so there is nothing to compare." \
			"" \
			'```' \
			"$output" \
			'```' \
			"" \
			"The schema and the runner contract are at the top of \`scripts/ci-conformance.sh\`."
	fi
	DIGEST="$output"
}

# Digest projections. The two id lists come back sorted so that `comm` can do the set arithmetic
# on them, which is what keeps the comparison per-test rather than per-count.
outcome_ids() {
	printf '%s\n' "$1" | awk -F'\t' '$1 == "outcome" { print $2 }' | sort
}

passing_ids() {
	printf '%s\n' "$1" | awk -F'\t' '$1 == "outcome" && $3 == "pass" { print $2 }' | sort
}

# awk rather than `grep -c`, which exits 1 on no match and would end the script under `set -e`
# on the most ordinary result this corpus has.
count_outcome() {
	printf '%s\n' "$2" |
		awk -F'\t' -v want="$1" '$1 == "outcome" && $3 == want { n++ } END { print n + 0 }'
}

count_lines() {
	printf '%s\n' "$1" | awk 'NF { n++ } END { print n + 0 }'
}

# The first ten lines of a list, and a count of the rest. `awk` rather than `head`, which closes
# the pipe early and would leave the pipeline reporting a signal under `set -o pipefail`.
preview() {
	local extra
	extra=$(($(count_lines "$1") - 10))
	printf '%s\n' "$1" | awk 'NR <= 10'
	if [[ $extra -gt 0 ]]; then
		printf '... and %s more\n' "$extra"
	fi
}

outcome_of() {
	printf '%s\n' "$2" | awk -F'\t' -v id="$1" '$1 == "outcome" && $2 == id { print $3 }'
}

# The recorded pass/fail counts of a digest, as `<passed> <failed>`.
verdicts() {
	printf '%s %s\n' "$(count_outcome pass "$1")" "$(count_outcome fail "$1")"
}

if [[ ! -f $BASELINE ]]; then
	fail \
		"There is no baseline at \`$BASELINE\`." \
		"" \
		"The baseline is committed on purpose: it is what makes a change in the number a" \
		"reviewable diff rather than a silent drift. Restore it, or record one with" \
		"\`scripts/ci-conformance.sh --update-baseline\`."
fi

read_digest "baseline" "$BASELINE"
BASELINE_DIGEST="$DIGEST"
BASELINE_IDS="$(outcome_ids "$BASELINE_DIGEST")"
BASELINE_TOTAL="$(count_lines "$BASELINE_IDS")"
read -r BASELINE_PASS BASELINE_FAIL <<<"$(verdicts "$BASELINE_DIGEST")"

# Resolve the runner. RUNNER stays empty when there is none, and is only ever expanded inside the
# branch that has already established it is not - `set -u` and an empty array do not mix on the
# bash 3.2 that ships with macOS.
RUNNER=()
if [[ -n ${KILN_CONFORMANCE_RUNNER:-} ]]; then
	read -r -a RUNNER <<<"$KILN_CONFORMANCE_RUNNER"
elif [[ -x $DEFAULT_RUNNER ]]; then
	RUNNER=("$DEFAULT_RUNNER")
fi

if [[ ${#RUNNER[@]} -eq 0 ]]; then
	# No runner, and a baseline that says nothing has ever run: the two agree, and the honest
	# report is "skipped". A baseline recording executions with no runner left to reproduce them
	# is the opposite - something that worked has been removed - and fails.
	if [[ $((BASELINE_PASS + BASELINE_FAIL)) -ne 0 ]]; then
		fail \
			"\`$BASELINE\` records $BASELINE_PASS passing and $BASELINE_FAIL failing tests, but no" \
			"runner could be found: \`$DEFAULT_RUNNER\` does not exist and \`KILN_CONFORMANCE_RUNNER\`" \
			"is unset. Results that cannot be reproduced cannot be defended, so this is a" \
			"regression in the gate itself rather than a corpus that scored zero."
	fi
	if [[ $UPDATE -eq 1 ]]; then
		fail \
			"\`--update-baseline\` needs a runner, and there is none. A baseline that was not" \
			"observed is not a baseline."
	fi
	kiln_say "### Conformance: skipped"
	kiln_say ""
	kiln_say "No runner executed the corpus: \`$DEFAULT_RUNNER\` does not exist and"
	kiln_say "\`KILN_CONFORMANCE_RUNNER\` is unset. \`$BASELINE\` records no test as having passed"
	kiln_say "or failed, so there is nothing for this run to have regressed against."
	kiln_say ""
	kiln_say "Reporting that as skipped rather than as a pass: zero of $BASELINE_TOTAL is not a pass"
	kiln_say "rate, it is the absence of one - the same reason \`kiln_report_has_measurements\`"
	kiln_say "refuses to let an empty benchmark report count as a performance pass."
	kiln_say "\`conformance/status.sh --gate\` is the check that stays red for exactly as long as"
	kiln_say "this is true."
	kiln_say ""
	kiln_say "This gate starts working the moment a runner exists. Point"
	kiln_say "\`KILN_CONFORMANCE_RUNNER\` at it, or add an executable \`$DEFAULT_RUNNER\`; the"
	kiln_say "interface it has to satisfy is the first block of \`scripts/ci-conformance.sh\`."
	exit 0
fi

mkdir -p "$(dirname -- "$RESULTS")"
# Remove any previous document first. Without this, a runner that dies before writing would leave
# the last run's numbers in place and this job would gate on them without noticing.
rm -f "$RESULTS"

runner_status=0
runner_output="$("${RUNNER[@]}" --out "$RESULTS" 2>&1)" || runner_status=$?

if [[ ! -f $RESULTS ]]; then
	fail \
		"\`${RUNNER[*]} --out $RESULTS\` exited $runner_status and wrote no results document." \
		"" \
		'```' \
		"$runner_output" \
		'```' \
		"" \
		"A non-zero exit status is fine on its own - failing tests are expected here - but a" \
		"missing document is not, because it is indistinguishable from a corpus that scored" \
		"zero. The interface this runner has to satisfy is at the top of this script."
fi

read_digest "current" "$RESULTS"
CURRENT_DIGEST="$DIGEST"

# The run has to cover the whole corpus. The source project's test262 runner would diff a subset
# against the full baseline, report every absent pass as a regression, and then overwrite the
# baseline with the subset; conformance/README.md records that as the lesson it is.
MANIFEST_IDS="$(kiln_conformance_manifest_digest "$MANIFEST" |
	awk -F'\t' '$1 == "test" { print $3 }' | sort)" ||
	fail "\`$MANIFEST\` could not be read, so the run cannot be checked for coverage."

CURRENT_IDS="$(outcome_ids "$CURRENT_DIGEST")"
MISSING="$(comm -23 <(printf '%s\n' "$MANIFEST_IDS") <(printf '%s\n' "$CURRENT_IDS"))"
UNKNOWN="$(comm -13 <(printf '%s\n' "$MANIFEST_IDS") <(printf '%s\n' "$CURRENT_IDS"))"

if [[ -n $MISSING ]]; then
	fail \
		"The run reported on $(count_lines "$CURRENT_IDS") of $(count_lines "$MANIFEST_IDS") tests." \
		"" \
		'```' \
		"$(preview "$MISSING")" \
		'```' \
		"" \
		"A partial run must never be compared against a full baseline: every absent test would" \
		"read as a regression, and re-baselining from it would delete the rest of the corpus."
fi

if [[ -n $UNKNOWN ]]; then
	fail \
		"The run reported tests that are not in \`$MANIFEST\`:" \
		"" \
		'```' \
		"$(preview "$UNKNOWN")" \
		'```' \
		"" \
		"Either the manifest or the runner's ids are wrong. Both halves have to name the same" \
		"tests for a per-test comparison to mean anything."
fi

read -r CURRENT_PASS CURRENT_FAIL <<<"$(verdicts "$CURRENT_DIGEST")"

REGRESSED="$(comm -23 <(passing_ids "$BASELINE_DIGEST") <(passing_ids "$CURRENT_DIGEST"))"
GAINED="$(comm -13 <(passing_ids "$BASELINE_DIGEST") <(passing_ids "$CURRENT_DIGEST"))"

# The document may name the runner that produced it; when it does not, the command this script
# actually invoked is the next most useful thing to put in the report.
RUNNER_LABEL="$(printf '%s\n' "$CURRENT_DIGEST" | awk -F'\t' '$1 == "meta" { print $3 }')"
if [[ $RUNNER_LABEL == "-" ]]; then
	RUNNER_LABEL="${RUNNER[*]}"
fi

kiln_say "### Conformance"
kiln_say ""
kiln_say "Corpus run by \`$RUNNER_LABEL\`, compared against \`$BASELINE\`."
kiln_say ""
kiln_say '```'
{
	printf '                  baseline   this run\n'
	for outcome in pass fail unsupported skipped not-yet-runnable; do
		printf '%-16s  %8s   %8s\n' "$outcome" \
			"$(count_outcome "$outcome" "$BASELINE_DIGEST")" \
			"$(count_outcome "$outcome" "$CURRENT_DIGEST")"
	done
	printf -- '----------------  --------   --------\n'
	printf '%-16s  %8s   %8s\n' "total" "$BASELINE_TOTAL" "$(count_lines "$CURRENT_IDS")"
	printf '%-16s  %8s   %8s\n' "executed" \
		"$((BASELINE_PASS + BASELINE_FAIL))" "$((CURRENT_PASS + CURRENT_FAIL))"
} | kiln_block
kiln_say '```'
kiln_say ""
kiln_say "\`executed\` counts the tests that reached a verdict - pass or fail. A test that is"
kiln_say "\`unsupported\`, \`skipped\` or \`not-yet-runnable\` produced no evidence either way, and"
kiln_say "is not in the denominator of anything."

if [[ -n $REGRESSED ]]; then
	kiln_say ""
	kiln_say "**$(count_lines "$REGRESSED") test(s) that passed at the baseline do not pass now.**"
	kiln_say ""
	kiln_say '```'
	while IFS= read -r id; do
		printf '%-40s  %s\n' "$id" "$(outcome_of "$id" "$CURRENT_DIGEST")"
	done <<<"$REGRESSED" | kiln_block
	kiln_say '```'
fi

if [[ -n $GAINED ]]; then
	kiln_say ""
	kiln_say "$(count_lines "$GAINED") test(s) pass that did not at the baseline:"
	kiln_say ""
	kiln_say '```'
	printf '%s\n' "$GAINED" | kiln_block
	kiln_say '```'
	kiln_say ""
	kiln_say "Re-record the baseline with \`scripts/ci-conformance.sh --update-baseline\` and commit"
	kiln_say "the result with the change that earned it. Until that lands, every test listed here is"
	kiln_say "free to start failing again without this gate noticing - which is the whole of what"
	kiln_say "the baseline buys."
fi

if [[ $UPDATE -eq 1 ]]; then
	# Write through a scratch file: a copy interrupted halfway must not leave a truncated
	# baseline behind, because the next run would compare against it and believe the result.
	scratch="$BASELINE.new"
	trap 'rm -f "$scratch"' EXIT
	cp "$RESULTS" "$scratch"
	mv "$scratch" "$BASELINE"
	kiln_say ""
	kiln_say "Wrote \`$BASELINE\`. Commit it with the change that made it necessary, and say in"
	kiln_say "the pull request why the new numbers are the right ones - especially if any of them"
	kiln_say "went down. A baseline that moves without an explanation is how a gate quietly stops"
	kiln_say "being one."
	exit 0
fi

if [[ -n $REGRESSED ]]; then
	kiln_say ""
	kiln_say "Failing the job. Fix the regressed tests, or re-record the baseline with"
	kiln_say "\`scripts/ci-conformance.sh --update-baseline\` and justify the smaller number in the"
	kiln_say "pull request. design.md §13 asks that a regression require an explicit justification;"
	kiln_say "this is the conformance half of that."
	exit 1
fi

if [[ -z $GAINED ]]; then
	kiln_say ""
	kiln_say "No change against \`$BASELINE\`."
fi
