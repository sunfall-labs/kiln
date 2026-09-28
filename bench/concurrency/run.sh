#!/usr/bin/env bash
#
# The concurrency benchmarks: kiln against node, bun and Go, one results table. See README.md.
#
# usage: bench/concurrency/run.sh [--quick] [--only start,messages,shared,tasks,pmap,server,realms]
#                                 [--out results.md] [--kiln path/to/kiln] [--go "go"]
#
# Refreshes the release `kiln` (mbx, never bare cargo), then hands over to run.py,
# which builds the native binaries (--profile native-release) and the Go program once, outside
# every measurement, runs every job in interleaved rounds, and writes
# bench/concurrency/results-<date>.md plus the raw records under target/bench-concurrency/.
# Go comes from PATH, else `mise exec go@latest` (which downloads into mise's cache only).
# Never gates. The numbers are properties of this machine on this afternoon.
set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$HERE/../.." && pwd)"
cd "$ROOT"

required=(node bun oha python3)
previous=""
for argument in "$@"; do
	if [[ $argument == --only=realms || ( $previous == --only && $argument == realms ) ]]; then
		required=(python3)
	fi
	previous="$argument"
done
for tool in "${required[@]}"; do
	command -v "$tool" >/dev/null || {
		echo "bench/concurrency: $tool is not on PATH" >&2
		exit 1
	}
done

kiln="$ROOT/target/release/kiln"
custom_kiln=0
for arg in "$@"; do
	if [[ ${prev:-} == --kiln ]]; then kiln="$arg"; custom_kiln=1; fi
	prev="$arg"
done
if (( ! custom_kiln )); then
	echo "bench/concurrency: building $kiln (mbx build --release -p kiln-cli)" >&2
	mbx build --release -p kiln-cli
elif [[ ! -x $kiln ]]; then
	echo "bench/concurrency: --kiln $kiln is not executable" >&2
	exit 1
fi

exec python3 "$HERE/run.py" "$@"
