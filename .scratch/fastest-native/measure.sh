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
command -v hyperfine >/dev/null || { echo "hyperfine is not on PATH" >&2; exit 1; }
WORK="$(mktemp -d "${TMPDIR:-/tmp}/kiln-ceiling.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
# No associative array: macOS ships bash 3.2 and `mise run` finds it (CLAUDE.md).
src_of() { case $1 in loop) echo bench/cpu/loop.mjs ;; fib) echo bench/cpu/fib.mjs ;; class-poly) echo bench/non-io/class-poly.mjs ;; esac; }
for b in loop fib class-poly; do
	src="$(src_of "$b")"
	"$KILN" --one-shot build "$src" --profile native-release --output "$WORK/$b.today" >/dev/null
	clang -O2 -std=c99 -o "$WORK/$b.ideal" ".scratch/fastest-native/$b.c" -lm
	want="$(node "$src")"
	for v in today ideal; do
		got="$("$WORK/$b.$v")"
		[[ $got == "$want" ]] || { echo "$b.$v answered $got, node answered $want" >&2; exit 1; }
	done
	echo "== $b (node whole process, today's native, hand-emitted C)"
	hyperfine -N -w 2 -r 7 "node $src" "$WORK/$b.today" "$WORK/$b.ideal" 2>/dev/null | grep -E 'Time \(mean'
	echo "   hand-emitted C, workload only (same basis as bench/harness.mjs): $("$WORK/$b.ideal" 2>&1 >/dev/null)"
done
