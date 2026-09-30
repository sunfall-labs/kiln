#!/usr/bin/env bash
#
# npm package compatibility corpus for kiln.
#
# Methodology mirrors porffor's npm-packages dashboard
# (https://porffor-packages.porch.page/): one small smoke test per package
# using its real API, run under Node for the expected stdout, compiled with
# `kiln --one-shot run --native`, and the binary's stdout compared byte for
# byte. The smoke tests here are kiln-authored (porffor's are not published).
#
# This is an informational benchmark, not a gate: it exits 0 even when
# packages fail. The committed baseline in results/ is the record to compare
# against, not a threshold to enforce.
#
# Usage:
#   ./run.sh                      # full corpus (360 packages)
#   ./run.sh ms mime              # subset by package name
#   TIMEOUT_S=300 ./run.sh        # per-package timeout (default 600)
#
# Prerequisites: node (>= 20), npm. Run `npm ci` (or `npm install`) in this
# directory first so the bare-specifier imports resolve.
#
# Two environment quirks the native backend needs, handled here:
# - it shells out to cargo/rustc (CARGO/RUSTC env vars, not just PATH)
# - it stages a ~2GB number-provider build under $TMPDIR, so TMPDIR points
#   at a scratch dir here instead of a small tmpfs /tmp
set -u

# Needs bash 4+ (associative arrays). Same re-exec pattern as scripts/gate.sh.
if [[ ${BASH_VERSINFO[0]} -lt 4 ]]; then
  for candidate in /opt/homebrew/bin/bash /usr/local/bin/bash "$(command -v bash)"; do
    if [[ -x $candidate ]] && [[ $("$candidate" -c 'echo ${BASH_VERSINFO[0]}') -ge 4 ]]; then
      exec "$candidate" "${BASH_SOURCE[0]}" "$@"
    fi
  done
  echo "npm-compat: needs bash 4 or newer" >&2
  exit 1
fi

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$HERE/../.." && pwd)"
KILN_BIN="${KILN_BIN:-$REPO_ROOT/target/release/kiln}"
TIMEOUT_S="${TIMEOUT_S:-600}"
OUT="$HERE/results"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ).$$"
RAW="$OUT/raw/$RUN_ID"

mkdir -p "$RAW" "$HERE/tmp"
export TMPDIR="$HERE/tmp"
export CARGO="${CARGO:-$(command -v cargo)}"
export RUSTC="${RUSTC:-$(command -v rustc)}"

if [[ ! -x "$KILN_BIN" ]]; then
  echo "building kiln (release) ..." >&2
  # mbx when available (house rule: shared cache + shared CPU budget across
  # concurrent checkouts); bare cargo only where mbx is not installed.
  if command -v mbx >/dev/null 2>&1; then
    (cd "$REPO_ROOT" && mbx build --release -p kiln-cli)
  else
    (cd "$REPO_ROOT" && cargo build --release -p kiln-cli)
  fi
  KILN_BIN="$REPO_ROOT/target/release/kiln"
fi
[[ -x "$KILN_BIN" ]] || { echo "no kiln binary at $KILN_BIN" >&2; exit 2; }

# A vanished corpus reads as a clean run; refuse instead of reporting zeros.
test_count=$(find "$HERE/tests" -name '*.test.*' | wc -l)
if ((test_count < 300)); then
  echo "npm-compat: corpus short ($test_count tests, expected ~360); refusing" >&2
  exit 2
fi

if [[ ! -d "$HERE/node_modules" ]]; then
  echo "no node_modules here; run 'npm ci' in $HERE first" >&2
  exit 2
fi

# The Python runner retains exact outputs and separates compilation from execution.
exec python3 "$HERE/runner.py" --kiln "$KILN_BIN" --raw "$RAW" \
  --timeout "$TIMEOUT_S" "$@"
