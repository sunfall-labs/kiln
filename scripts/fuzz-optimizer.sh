#!/usr/bin/env bash
#
# Differential fuzzing of the optimizer across Node and Kiln (Step 2.2).
#
# Runs typed-ish generated programs under KILN_SPECULATE=0, 1, and exit
# compared against Node.js and Kiln interpreter.
set -euo pipefail

DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DIR"

CASES="${1:-20}"
echo "Running optimizer differential fuzzing ($CASES cases)..."
cargo run --manifest-path fuzz/Cargo.toml --release --bin speculate_soak -- --cases "$CASES"
