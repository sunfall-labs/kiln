#!/usr/bin/env bash
set -euo pipefail

: "${KILN_BASELINE_BIN:?set to the baseline kiln binary}"
: "${KILN_CANDIDATE_BIN:?set to the candidate kiln binary}"
baseline_bin="$(realpath "$KILN_BASELINE_BIN")"
candidate_bin="$(realpath "$KILN_CANDIDATE_BIN")"
repo="$(git rev-parse --show-toplevel)"
if [[ -n "${KILN_BENCH_ROOT:-}" ]]; then
    run_root="$KILN_BENCH_ROOT"
else
    run_root="$(mktemp -d "$repo/docs/issues/mass-bootstrap/run.XXXXXX")"
fi
project="$run_root/project"
mkdir -p "$project/out"
printf '%s\n' 'export function main() { console.log("bootstrap measure"); }' 'main();' > "$project/main.ts"
cd "$project"

cc="${CC:-$HOME/.cache/kiln/clang-18/usr/bin/clang-18}"
ld_path="${LD_LIBRARY_PATH:-$HOME/.cache/kiln/clang-18/usr/lib/x86_64-linux-gnu:$HOME/.cache/kiln/clang-18/usr/lib/llvm-18/lib}"
jobs="${KILN_JOBS:-4}"
base_socket="$run_root/baseline.sock"
candidate_socket="$run_root/candidate.sock"

build() {
    local label="$1" binary="$2" socket="$3" output="$4"
    /usr/bin/time -p -o "$run_root/$label.time" env \
        KILN_SOCKET="$socket" CC="$cc" LD_LIBRARY_PATH="$ld_path" KILN_JOBS="$jobs" \
        "$binary" build main.ts --profile native-release --output "out/$output" \
        > "$run_root/$label.log" 2>&1
    printf '%-16s ' "$label"
    rg 'native build|timing          emit' "$run_root/$label.log" | tr '\n' ' '
    rg '^real ' "$run_root/$label.time"
}

# First requests include compilation and provider setup. The following samples use warm daemons.
build baseline-first "$baseline_bin" "$base_socket" baseline
build baseline-warm "$baseline_bin" "$base_socket" baseline
build candidate-first "$candidate_bin" "$candidate_socket" candidate
build candidate-warm "$candidate_bin" "$candidate_socket" candidate

for sample in 1 2 3; do
    build "baseline-$sample" "$baseline_bin" "$base_socket" baseline
    build "candidate-$sample" "$candidate_bin" "$candidate_socket" candidate
done

env KILN_SOCKET="$base_socket" "$baseline_bin" daemon stop >/dev/null 2>&1 || true
env KILN_SOCKET="$candidate_socket" "$candidate_bin" daemon stop >/dev/null 2>&1 || true
printf 'full per-request logs and wall times: %s\n' "$run_root"
