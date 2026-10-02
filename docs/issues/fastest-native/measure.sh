#!/usr/bin/env bash
# The ceiling measurement behind docs/architecture/fastest-native-path.md.
#
# Builds the three benchmarks natively with the kiln in this worktree, compiles the hand-emitted
# C beside this script with the same clang flags the native-release profile uses, and times both
# against node. The hand-emitted C is what the middle-end described in the note should produce
# for these programs; it links nothing from the prelude, so it is a ceiling and not a prediction.
#
# Refuses to print a time for a binary whose output differs from node's: a fast wrong answer reads
# exactly like a win.
set -euo pipefail
cd "$(dirname -- "${BASH_SOURCE[0]}")/../.."
KILN="${KILN_BENCH_KILN:-target/release/kiln}"
[[ -x $KILN ]] || { echo "no kiln at $KILN; mbx build --release -p kiln-cli" >&2; exit 1; }
# A number taken while other agents are compiling is a clock reading. On 2026-09-04 the same
# loop binary read 156 ms and 296 ms two runs apart at a load average near 4, and an identical
# emitted body was called a regression. Refuse rather than print; KILN_MEASURE_ANYWAY=1 overrides.
load="$(sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}' || awk '{print $1}' /proc/loadavg)"
if [[ -z ${KILN_MEASURE_ANYWAY:-} ]] && awk -v l="$load" 'BEGIN{exit !(l > 1.5)}'; then
	echo "load average is $load; wait for the machine to go quiet, or set KILN_MEASURE_ANYWAY=1" >&2
	exit 1
fi
echo "load average $load"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/kiln-ceiling.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
# No associative array: macOS ships bash 3.2 and `mise run` finds it (CLAUDE.md).
src_of() { case $1 in loop) echo bench/cpu/loop.mjs ;; fib) echo bench/cpu/fib.mjs ;; class-poly) echo bench/non-io/class-poly.mjs ;; esac; }
for b in loop fib class-poly; do
	src="$(src_of "$b")"
	"$KILN" --one-shot build "$src" --profile native-release --output "$WORK/$b.today" >/dev/null
	clang -O2 -std=c99 -D_POSIX_C_SOURCE=199309L -o "$WORK/$b.ideal" "docs/issues/fastest-native/$b.c" -lm
	# bench/harness.mjs prints one JSON record per run: the checksum decides agreement and
	# warm_median is the in-process number, the same basis for node and for today's native.
	field() { python3 -c 'import sys,json; r=[l for l in sys.stdin.read().splitlines() if l.startswith("{")][-1]; print(json.loads(r)[sys.argv[1]])' "$1"; }
	want="$(node "$src" | field checksum)"
	today="$("$WORK/$b.today")"
	[[ $(field checksum <<<"$today") == "$want" ]] || { echo "$b.today checksum differs from node's $want" >&2; exit 1; }
	ideal_out="$("$WORK/$b.ideal" 2>"$WORK/$b.ideal.err")"
	[[ $ideal_out == "$want" ]] || { echo "$b.ideal answered $ideal_out, node answered $want" >&2; exit 1; }
	node_warm="$(node "$src" | field warm_median)"
	today_warm="$(field warm_median <<<"$today")"
	ideal_warm="$(sed -n 's/workload median \(.*\) ms/\1/p' "$WORK/$b.ideal.err")"
	printf '%-12s node warm %8.3f ms   native today warm %8.3f ms   hand-emitted C warm %8.3f ms\n' "$b" "$node_warm" "$today_warm" "$ideal_warm"
done
