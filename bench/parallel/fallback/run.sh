#!/usr/bin/env bash
#
# Parallel issue 15: each parallel method with its pool declined, against its sequential method,
# natively, at 1M elements, per array kind. The pool is declined at compile time (the callbacks
# read a reassigned capture, `captures-reassigned`) for "numeric" and "typed", and at run time
# for "mixed" (an array holding a non-number). The two columns should be within noise.
#
# usage: bench/parallel/fallback/run.sh [path/to/kiln]
#
# Best of 7 warm calls in one process, the first call apart; not the interleaved-rounds harness
# of bench/parallel, which measures the pooled calls. Each program is one pair so the module stays
# under the release profile's specialization limits, as a benchmark's workload module does.
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$HERE/../../.." && pwd)"
kiln="${1:-$ROOT/target/release/kiln}"
work="$ROOT/target/bench-parallel/fallback"
mkdir -p "$work"
cd "$ROOT"
printf '%-8s %-8s %12s %12s\n' kind method "X (ms)" "parallelX"
for method in map filter reduce sort; do
	"$kiln" --one-shot --node build "bench/parallel/fallback/$method.mjs" --profile native-release \
		--output "$work/$method" >"$work/$method.build.log" 2>&1 ||
		{ echo "fallback: $method did not build; see $work/$method.build.log" >&2; exit 1; }
	for kind in numeric mixed typed; do
		"$work/$method" "$kind" | awk -v m="$method" '
			{ ms[NR] = $3; out[NR] = $4 }
			END {
				if (out[1] != out[2]) { printf "fallback: %s %s answers differ\n", $1, m > "/dev/stderr"; exit 1 }
				printf "%-8s %-8s %12s %12s\n", $1, m, ms[1], ms[2]
			}'
	done
done
