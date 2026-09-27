#!/usr/bin/env bash
# Check or regenerate the committed Test262 chunk baseline.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"
UPDATE=""
SUITE=routine
while [[ $# -gt 0 ]]; do
    case "$1" in
        --update) UPDATE=--update; SUITE=full; shift ;;
        --suite) [[ $# -ge 2 ]] || { echo "--suite needs routine, extended or full" >&2; exit 2; }; SUITE="$2"; shift 2 ;;
        *) echo "usage: test262-baseline.sh [--suite routine|extended|full] [--update]" >&2; exit 2 ;;
    esac
done
KILN_BIN="${KILN_TEST262_KILN:-target/release/kiln}"
[[ -x $KILN_BIN ]] || { echo "test262-baseline.sh: no kiln binary at $KILN_BIN" >&2; exit 1; }
WORK="$(mktemp -d "${TMPDIR:-/tmp}/kiln-test262-baseline.XXXXXXXX")"
LOG_ROOT="${KILN_TEST262_LOG_DIR:-$PWD/target/test262-baseline-logs}"
mkdir -p "$LOG_ROOT"; LOG_DIR="$(mktemp -d "$LOG_ROOT/run.XXXXXXXX")"
trap 'rm -rf "$WORK"' EXIT
export KILN_TEST262_LOG_DIR="$LOG_DIR"
ARGS=(baseline --suite "$SUITE" --kiln "$KILN_BIN" --baseline conformance/test262-baseline.json --work "$WORK" --logdir "$LOG_DIR")
[[ -n ${KILN_TEST262_REPORT:-} ]] && ARGS+=(--report "$KILN_TEST262_REPORT")
[[ -n ${KILN_TEST262_CHECKOUT:-} ]] && ARGS+=(--checkout "$KILN_TEST262_CHECKOUT")
[[ -n $UPDATE ]] && ARGS+=(--update)
python3 "$PWD/scripts/test262_corpus.py" "${ARGS[@]}"
