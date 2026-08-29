#!/usr/bin/env bash
#
# Print the conformance corpus inventory, and whatever has actually been recorded about it.
#
# What this script can honestly say, and what it must never say:
#
# Manifest status and recorded execution are intentionally different facts. A summary line such
# as "37 tests, 0 failures" would be true arithmetic and a lie when some tests were skipped or
# never ran: it says the corpus passed when the corpus produced no such evidence.
#
# This is the same failure mode scripts/lib.sh already refuses to allow. kiln_report_has_
# measurements exists because a benchmark report with no measurements in it must be called
# skipped rather than passed - "a green tick on an empty measurement is worse than no
# measurement at all". The same reasoning applies with more force here, because a conformance
# suite is the thing people most want to read a green tick off. So no count of failures is
# printed for tests that did not run, and no pass rate is ever taken over anything but the tests
# that actually reached a verdict.
#
# It reads two files, and the distinction between them is the point. manifest.toml records what
# the corpus *is*: which tests exist, what each one exercises, and what is known about it without
# running it. conformance/results/baseline.json records what a runner *observed*, and it is the
# number scripts/ci-conformance.sh gates on. This script still executes nothing itself; when
# both files disagree about a test, the observation wins, because it is the only one of the two
# that is evidence.
#
# It also refuses to be used as a hollow gate. `--gate` exits non-zero for exactly as long as
# nothing has run, and afterwards for as long as anything is failing, so that wiring this into
# CI early produces an honest red rather than a vacuous green.
#
# It sources scripts/lib.sh rather than standing alone, for two reasons. The narrower one is
# style: every script in the repository that prints for a human does so through kiln_say, and
# duplicating that here would be the beginning of a second convention. The wider one is that
# kiln_block is what mirrors this output into the GitHub job summary, and the job summary is the
# single place where a false "0 failures" would do the most damage - so this script's honesty is
# worth exactly as much as its reaching that surface. The cost is that conformance/ depends on
# scripts/, which is a real coupling, but it is a one-directional one to a handful of helpers -
# two of which, the manifest and results readers, are shared with the gate precisely so that the
# two cannot come to different conclusions about the same files.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR/../scripts
# shellcheck source=../scripts/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../scripts/lib.sh"

CONFORMANCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="$CONFORMANCE_DIR/manifest.toml"
# The same variable scripts/ci-conformance.sh compares against, so that "what the gate is
# holding" and "what this page reports" cannot drift apart. Point it at a fresh run's document
# to inspect one before it is recorded.
RECORDED="${KILN_CONFORMANCE_BASELINE:-$CONFORMANCE_DIR/results/baseline.json}"

GATE=0
for arg in "$@"; do
	case "$arg" in
	--gate)
		GATE=1
		;;
	-h | --help)
		cat <<-'USAGE'
			usage: conformance/status.sh [--gate]

			  (no flags)  Print the corpus inventory and every test's recorded state.
			  --gate      Same output, but exit non-zero while the corpus cannot run at all, and
			              afterwards while any test is failing. Use this if you wire the corpus
			              into CI before a runner exists, so that the job is honestly red rather
			              than vacuously green.

			environment:
			  KILN_CONFORMANCE_BASELINE  recorded results to read, if any
		USAGE
		exit 0
		;;
	*)
		echo "status.sh: unknown argument '$arg' (try --help)" >&2
		exit 2
		;;
	esac
done

if [[ ! -f $MANIFEST ]]; then
	echo "status.sh: no manifest at $MANIFEST" >&2
	exit 2
fi

DIGEST="$(kiln_conformance_manifest_digest "$MANIFEST")"

# A recorded results file that cannot be read is a hard error rather than a quiet fall back to
# the manifest. Reporting "nothing has run" for a corpus that has run is the same class of false
# statement this script exists to prevent, only pointing the other way.
RECORDED_DIGEST=""
if [[ -f $RECORDED ]]; then
	if ! RECORDED_DIGEST="$(kiln_conformance_results_digest "$RECORDED")"; then
		exit 2
	fi
fi

corpus_line="$(printf '%s\n' "$DIGEST" | grep '^corpus	' || true)"
CORPUS_NAME="$(printf '%s\n' "$corpus_line" | cut -f2)"
CORPUS_COMMIT="$(printf '%s\n' "$corpus_line" | cut -f3)"

TESTS="$(printf '%s\n' "$DIGEST" | grep '^test	' || true)"
FIXTURES="$(printf '%s\n' "$DIGEST" | grep '^fixture	' || true)"

count() { printf '%s\n' "$1" | grep -c . || true; }

TOTAL="$(count "$TESTS")"
GUARD="$(count "$(printf '%s\n' "$TESTS" | grep '	guard	' || true)")"
EFFECT="$(count "$(printf '%s\n' "$TESTS" | grep '	effect	' || true)")"
REGRESS="$(count "$(printf '%s\n' "$TESTS" | grep '	regress	' || true)")"
FIXTURE_COUNT="$(count "$FIXTURES")"

# One line per test: its id, the outcome that is true of it, and its feature tags. A recorded
# observation displaces the manifest's status, which is the only ordering that makes sense - the
# manifest says what nobody has checked yet, the results say what somebody did.
MERGED="$(awk -F'\t' '
	NR == FNR { if ($1 == "outcome") { recorded[$2] = $3 } ; next }
	$1 == "test" { print $3 "\t" (($3 in recorded) ? recorded[$3] : $4) "\t" $5 }
' <(printf '%s\n' "$RECORDED_DIGEST") <(printf '%s\n' "$DIGEST"))"

# The gate and this page have to be talking about the same tests for either to mean anything.
STRAY="$(awk -F'\t' '
	NR == FNR { if ($1 == "test") { known[$3] = 1 } ; next }
	$1 == "outcome" && !($2 in known) { print $2 }
' <(printf '%s\n' "$DIGEST") <(printf '%s\n' "$RECORDED_DIGEST"))"
if [[ -n $STRAY ]]; then
	echo "status.sh: $RECORDED records tests that are not in the manifest:" >&2
	printf '%s\n' "$STRAY" >&2
	exit 2
fi

outcome_count() {
	printf '%s\n' "$MERGED" | awk -F'\t' -v want="$1" '$2 == want { n++ } END { print n + 0 }'
}

# Whole percent. The exact counts are printed directly underneath it, so a decimal place here
# would add precision to a figure whose only job is to be read at a glance.
percent() {
	awk -v part="$1" -v whole="$2" 'BEGIN { printf "%.0f", (whole ? 100 * part / whole : 0) }'
}

PASSED="$(outcome_count pass)"
FAILED="$(outcome_count fail)"
UNSUPPORTED="$(outcome_count unsupported)"
SKIPPED="$(outcome_count skipped)"
UNRUNNABLE="$(outcome_count not-yet-runnable)"
EXECUTED=$((PASSED + FAILED))

kiln_say "### JavaScript conformance corpus"
kiln_say ""
kiln_say "Corpus \`$CORPUS_NAME\`, ported from source commit \`$CORPUS_COMMIT\`."
kiln_say ""
kiln_say '```'
{
	printf 'group    tests  description\n'
	printf 'guard    %5s  standalone probes, one JavaScript behaviour each\n' "$GUARD"
	printf 'effect   %5s  real Effect-TS programs, whole-library at once\n' "$EFFECT"
	printf 'regress  %5s  Kiln-native regressions, one fixed defect each\n' "$REGRESS"
	printf '                %s committed input fixture(s), not counted as tests\n' "$FIXTURE_COUNT"
	printf -- '-------  -----\n'
	printf 'total    %5s\n' "$TOTAL"
} | kiln_block
kiln_say '```'
kiln_say ""

if [[ -n $RECORDED_DIGEST ]]; then
	meta_line="$(printf '%s\n' "$RECORDED_DIGEST" | grep '^meta	' || true)"
	recorded_on="$(printf '%s\n' "$meta_line" | cut -f2)"
	recorded_by="$(printf '%s\n' "$meta_line" | cut -f3)"
	if [[ $recorded_by == "-" ]]; then
		# No `runner` field. That is what a results file transcribed from the manifest looks
		# like, as opposed to one an execution produced, and the difference is worth saying out
		# loud rather than rendering as an empty column.
		kiln_say "Recorded state read from \`${RECORDED#"$CONFORMANCE_DIR/"}\`, dated $recorded_on."
		kiln_say "It names no runner, so nothing in it was observed by one."
	else
		kiln_say "Recorded results read from \`${RECORDED#"$CONFORMANCE_DIR/"}\`, dated"
		kiln_say "$recorded_on, produced by \`$recorded_by\`."
	fi
else
	kiln_say "There is no recorded results file at \`$RECORDED\`, so every state below is the"
	kiln_say "manifest's own, and nothing below is an observation."
fi
kiln_say ""

# The whole point of this script. Do not soften it, and do not take a pass rate over anything but
# $EXECUTED: a pass rate over tests that did not run is not a small inaccuracy, it is the
# specific false statement this exists to prevent.
if [[ $EXECUTED -eq 0 && $UNRUNNABLE -eq $TOTAL ]]; then
	kiln_say "**Nothing in this corpus has run.** All $TOTAL tests are \`not-yet-runnable\`."
	kiln_say ""
	kiln_say "The recorded document names no completed runner result for any test:"
elif [[ $EXECUTED -eq 0 ]]; then
	# A runner exists and reached none of them. Distinct from the state above, and worth saying
	# separately: "all not-yet-runnable" is a compiler that cannot start, while this is a corpus
	# every test of which was refused.
	kiln_say "**No test in this corpus reached a verdict.** Nothing passed, nothing failed, and"
	kiln_say "every test was skipped, unsupported, or has still never been run:"
else
	kiln_say "**$PASSED of the $EXECUTED tests that ran pass** ($(percent "$PASSED" "$EXECUTED")%)."
fi

kiln_say ""
kiln_say '```'
{
	printf 'executed          %s\n' "$EXECUTED"
	printf 'passed            %s\n' "$PASSED"
	printf 'failed            %s\n' "$FAILED"
	printf 'unsupported       %s\n' "$UNSUPPORTED"
	printf 'skipped           %s\n' "$SKIPPED"
	printf 'not yet runnable  %s\n' "$UNRUNNABLE"
} | kiln_block
kiln_say '```'
kiln_say ""

if [[ $EXECUTED -eq 0 ]]; then
	kiln_say "Read those zeroes as \"no evidence\", not as \"no problems\". Reporting this as"
	kiln_say "\"$TOTAL tests, 0 failures\" would be true arithmetic and a false claim, in the same"
	kiln_say "way an empty benchmark report is not a performance pass - see"
	kiln_say "\`kiln_report_has_measurements\` in \`scripts/lib.sh\`."
else
	kiln_say "The rate above is over the $EXECUTED tests that reached a verdict, and nothing else."
	if [[ $TOTAL -gt $EXECUTED ]]; then
		kiln_say "The other $((TOTAL - EXECUTED)) produced no evidence either way, so counting them"
		kiln_say "as either passing or failing would be an invention."
	fi
fi

kiln_say ""
kiln_say "Per test:"
kiln_say ""
kiln_say '```'
printf '%s\n' "$MERGED" | while IFS=$'\t' read -r id outcome features; do
	printf '%-18s  %-40s  %s\n' "$outcome" "$id" "$features"
done | kiln_block
kiln_say '```'
kiln_say ""
kiln_say "\`features\` follows docs/design.md §9 and is what a changed-test selector will key on."
kiln_say "Expected output for every test is recorded in \`conformance/manifest.toml\`; see"
kiln_say "\`conformance/README.md\` for how it was obtained and how the runner records results."

if [[ $GATE -eq 1 && $EXECUTED -eq 0 ]]; then
	kiln_say ""
	kiln_say "Failing \`--gate\`: a suite that cannot run cannot pass."
	exit 1
fi

if [[ $GATE -eq 1 && $FAILED -gt 0 ]]; then
	kiln_say ""
	kiln_say "Failing \`--gate\`: $FAILED test(s) ran and did not pass."
	exit 1
fi
