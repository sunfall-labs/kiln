#!/usr/bin/env bash
#
# Check the whole-program facts baseline for benchmarks and bundles, or regenerate with --update.
#
# Snapshotting inferred types, sites, and closed-world verdicts ensures that changes to
# whole-program inference produce a reviewable, chunk-by-chunk diff.
#
# usage: scripts/facts-baseline.sh [options] [targets...]
#   --check               verify current facts against baseline (default)
#   --update              regenerate/update the committed baseline JSON
#   --baseline <path>     custom baseline JSON path (default: bench/results/facts-baseline.json)
#   --all                 run on all benchmarks and thirdparty bundles
#   --explain             print human-readable fixpoint explanation
#   --json                output facts as JSON
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

KILN_FACTS_BIN="${KILN_FACTS_BIN:-}"
if [[ -z $KILN_FACTS_BIN ]]; then
	if [[ -x "$(kiln_repo_root)/target/release/kiln-facts" ]]; then
		KILN_FACTS_BIN="$(kiln_repo_root)/target/release/kiln-facts"
	elif [[ -x "$(kiln_repo_root)/target/debug/kiln-facts" ]]; then
		KILN_FACTS_BIN="$(kiln_repo_root)/target/debug/kiln-facts"
	else
		kiln_cargo build --release --locked -p kiln-cli >&2
		KILN_FACTS_BIN="$(kiln_repo_root)/target/release/kiln-facts"
	fi
fi

"$KILN_FACTS_BIN" "$@"
