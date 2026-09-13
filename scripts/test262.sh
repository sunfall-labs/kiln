#!/usr/bin/env bash
# Run the pinned upstream Test262 tree through the shared corpus policy.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
usage() { echo 'usage: scripts/test262.sh --out <path> [--path <relative subtree>]' >&2; }
[[ $# -eq 2 || $# -eq 4 ]] || { usage; exit 2; }
[[ $1 == --out && -n $2 ]] || { usage; exit 2; }
RESULTS="$2"; TEST_PATH=""
if [[ $# -eq 4 ]]; then [[ $3 == --path && -n $4 ]] || { usage; exit 2; }; TEST_PATH="$4"; fi
REPO_ROOT="$(kiln_repo_root)"
[[ $RESULTS == /* ]] || RESULTS="$REPO_ROOT/$RESULTS"
KILN_BIN="${KILN_TEST262_KILN:-$REPO_ROOT/target/release/kiln}"
[[ -x $KILN_BIN ]] || { echo "test262.sh: Kiln CLI binary is not executable: $KILN_BIN" >&2; exit 1; }
LOG_ROOT="${KILN_TEST262_LOG_DIR:-$REPO_ROOT/target/test262-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="$(mktemp -d "$LOG_ROOT/run.XXXXXXXX")"
RAW="$(mktemp "${TMPDIR:-/tmp}/kiln-test262-raw.XXXXXXXX.json")"
trap 'rm -f "$RAW"' EXIT
export KILN_TEST262_LOG_DIR="$LOG_DIR"
ARGS=(run --kiln "$KILN_BIN" --raw "$RAW" --out "$RESULTS" --log "$LOG_DIR/test262.log")
[[ -n ${KILN_TEST262_CHECKOUT:-} ]] && ARGS+=(--checkout "$KILN_TEST262_CHECKOUT")
[[ -n $TEST_PATH ]] && ARGS+=(--path "$TEST_PATH")
python3 "$REPO_ROOT/scripts/test262_corpus.py" "${ARGS[@]}"
