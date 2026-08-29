#!/usr/bin/env bash
#
# Run the timing harness and compare it against the committed baseline.
#
# What this can honestly gate on, and what it cannot:
#
# `kiln-bench --compare` reports two kinds of regression. One is wall clock, which on a
# GitHub-hosted runner is worth very little - the runner shares a physical host with other
# tenants, and the committed baseline was recorded on somebody's development machine, which is
# a different architecture again. The harness says so itself, printing a note when the baseline
# came from a different machine than the run. Gating on that number would produce red builds
# that mean nothing and teach everyone to hit re-run.
#
# The other is the recompute counters from kiln_proto::Stats: how many queries the pipeline had
# to recompute for a given edit. That is a property of the dependency graph, it is identical on
# every machine, and it is the thing design.md §2 and §3 are actually about. It is worth
# blocking a merge on, and it is what this job blocks on.
#
# The mechanism is KILN_PERF_THRESHOLD, which is handed to `--compare` as its wall-clock
# regression threshold. The CI default is large enough to switch wall-clock gating off, leaving
# the counter comparison as the only thing that can fail the job; the per-measurement percentage
# deltas are still printed, so the trend stays visible in the job summary. A developer running
# `kiln-bench --compare bench/results/baseline.json` on their own machine gets the harness's own
# 15% default, which is where a wall-clock threshold means something.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

BASELINE="${KILN_BENCH_BASELINE:-bench/results/baseline.json}"
RESULTS="${KILN_BENCH_RESULTS:-bench/results/latest.json}"
THRESHOLD="${KILN_PERF_THRESHOLD:-10000}"
BLOCKING="${KILN_PERF_BLOCKING:-1}"

BENCH="$(kiln_build_bench)"

status=0
if [[ -f $BASELINE ]]; then
	output="$("$BENCH" --out "$RESULTS" --compare "$BASELINE" --threshold "$THRESHOLD" 2>&1)" ||
		status=$?
else
	output="$("$BENCH" --out "$RESULTS" 2>&1)" || status=$?
fi

# Exit 2 is the harness saying it could not run at all - a missing workload, or a baseline
# recorded under an older report schema. That is a broken job rather than a slow one, so it
# fails whatever the blocking policy says.
if [[ $status -eq 2 ]]; then
	kiln_say "### Performance: the harness could not run"
	kiln_say ""
	kiln_say '```'
	printf '%s\n' "$output" | kiln_block
	kiln_say '```'
	exit 1
fi

if ! kiln_report_has_measurements "$RESULTS"; then
	kiln_say "### Performance: skipped"
	kiln_say ""
	kiln_say "\`kiln-bench\` recorded no measurements, so there is nothing to compare."
	kiln_say "Reporting that as skipped rather than as a pass - a green tick on an empty"
	kiln_say "measurement would be a lie."
	exit 0
fi

kiln_say "### Performance"
kiln_say ""
kiln_say '```'
printf '%s\n' "$output" | kiln_block
kiln_say '```'
kiln_say ""

if [[ ! -f $BASELINE ]]; then
	kiln_say "There is no committed baseline at \`$BASELINE\`, so the run above was only"
	kiln_say "recorded, not compared. Seed one with \`scripts/update-baseline.sh\` and commit it."
	exit 0
fi

if [[ $status -eq 0 ]]; then
	kiln_say "No regression against \`$BASELINE\`."
	exit 0
fi

kiln_say "**\`kiln-bench --compare\` reported a regression.**"
kiln_say ""
kiln_say "At the threshold this job uses (${THRESHOLD}%), wall clock cannot trip the gate, so"
kiln_say "this is a recompute-count regression: an edit now invalidates more queries than it"
kiln_say "did at the baseline. That is machine-independent, and it is the property design.md §3"
kiln_say "exists to protect."
kiln_say ""
kiln_say "§13 asks that a regression require explicit justification. Either fix it, or"
kiln_say "re-baseline with \`scripts/update-baseline.sh\` and say in the pull request why the"
kiln_say "new numbers are the right ones. \`$BASELINE\` is committed precisely so that"
kiln_say "re-baselining is a reviewable diff rather than a silent drift."

if [[ $BLOCKING == 1 ]]; then
	exit 1
fi

kiln_say ""
kiln_say "Not failing the job: KILN_PERF_BLOCKING is \`$BLOCKING\`."
