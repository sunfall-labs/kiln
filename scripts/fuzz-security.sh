#!/usr/bin/env bash
# Bounded Bolero RNG campaigns. Each target also supports coverage-guided cargo-bolero runs.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"
export KILN_FUZZ_ITERATIONS=${KILN_FUZZ_ITERATIONS:-10000}
export KILN_JOBS=${KILN_JOBS:-2}
export RAYON_NUM_THREADS=${RAYON_NUM_THREADS:-$KILN_JOBS}
[[ $KILN_FUZZ_ITERATIONS =~ ^[1-9][0-9]*$ ]] || {
    echo 'KILN_FUZZ_ITERATIONS must be positive' >&2
    exit 2
}
kiln_cargo test --locked --manifest-path fuzz/security/Cargo.toml --tests -- --test-threads=1
