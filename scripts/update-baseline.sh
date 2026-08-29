#!/usr/bin/env bash
#
# Regenerate the committed performance baseline.
#
# This is a deliberate, manual act. design.md §13 says a regression should require explicit
# justification, and the mechanism for that is this file being in the repository: moving the
# baseline shows up as a diff a reviewer has to accept, next to the change that caused it.
# Nothing in CI runs this.
#
# Run it on the quietest machine you have, not on a laptop that is also building something.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

BASELINE="${KILN_BENCH_BASELINE:-bench/results/baseline.json}"

BENCH="$(kiln_build_bench)"
mkdir -p "$(dirname -- "$BASELINE")"

# Write to a scratch file first: a harness that fails halfway must not leave a truncated
# baseline behind, because the next CI run would compare against it and believe the result.
scratch="$BASELINE.new"
trap 'rm -f "$scratch"' EXIT

"$BENCH" --out "$scratch"
if ! kiln_report_has_measurements "$scratch"; then
	echo "kiln-bench recorded no measurements; refusing to write an empty baseline" >&2
	exit 1
fi
mv "$scratch" "$BASELINE"

echo "wrote $BASELINE"
echo
echo "Commit it with the change that made it necessary, and say in the pull request why the"
echo "new numbers are the right ones. A baseline that moves without an explanation is how a"
echo "performance gate quietly stops being one."
