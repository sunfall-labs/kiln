#!/usr/bin/env bash
#
# The parallel built-ins benchmarks: kiln's parallel family against node, bun, a node
# worker_threads pool and Go, one results table. See README.md.
#
# usage: bench/parallel/run.sh [--quick] [--only map-heavy,sort-array,...]
#                              [--out results.md] [--kiln path/to/kiln] [--go "go"]
#
# Refreshes the release `kiln` before measuring (mbx, never bare cargo), then hands over to run.py,
# which builds the kiln programs (--profile native-release, --explain-parallel) and the Go program
# once, outside every measurement, runs every job in interleaved rounds, and writes
# bench/parallel/results-<date>.md plus the raw records under target/bench-parallel/.
# Go comes from PATH, else `mise exec go@latest` (which downloads into mise's cache only).
# Never gates. The numbers are properties of this machine on this afternoon.
set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$HERE/../.." && pwd)"
cd "$ROOT"

for tool in node bun python3; do
	command -v "$tool" >/dev/null || {
		echo "bench/parallel: $tool is not on PATH" >&2
		exit 1
	}
done

kiln="$ROOT/target/release/kiln"
explicit_kiln=0
for arg in "$@"; do
	if [[ ${prev:-} == --kiln ]]; then kiln="$arg"; explicit_kiln=1; fi
	prev="$arg"
done
if (( ! explicit_kiln )); then
	echo "bench/parallel: refreshing $kiln (mbx build --release -p kiln-cli)" >&2
	mbx build --release -p kiln-cli
fi
if [[ ! -x $kiln ]]; then
	echo "bench/parallel: kiln is not executable: $kiln" >&2
	exit 1
fi

exec python3 "$HERE/run.py" "$@"
