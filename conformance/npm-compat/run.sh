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
RAW="$OUT/raw"

mkdir -p "$RAW" "$HERE/tmp"
export TMPDIR="$HERE/tmp"
export CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
export RUSTC="${RUSTC:-$HOME/.cargo/bin/rustc}"

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

# usage: run.sh [name-filter...]   (empty = all)
want=("$@")

declare -A VER=()      # name -> version
declare -A SAFE=()     # name -> test-file stem
declare -A PORF=()     # name -> porffor status
ORDER=()
while IFS= read -r line; do
  # NOTE: bash `read` collapses consecutive tab delimiters, so an empty
  # detail column would shift `safe` out. Stuff a placeholder first.
  line="${line//$'\t\t'/$'\t<empty>\t'}"
  IFS=$'\t' read -r name ver pstatus _detail safe <<< "$line"
  [[ "$_detail" == "<empty>" ]] && _detail=""
  [[ -z "$name" ]] && continue
  ORDER+=("$name")
  VER["$name"]="$ver"
  PORF["$name"]="$pstatus"
  SAFE["$name"]="$safe"
done < "$HERE/packages.tsv"

result_lines=()
for name in "${ORDER[@]}"; do
  if ((${#want[@]} > 0)); then
    skip=1
    for w in "${want[@]}"; do [[ "$name" == "$w" ]] && skip=0; done
    ((skip)) && continue
  fi
  stem="${SAFE[$name]:-$name}"
  # locate test file (.js or .mjs)
  testfile=""
  for ext in js mjs; do
    [[ -f "$HERE/tests/$stem.test.$ext" ]] && testfile="$HERE/tests/$stem.test.$ext"
  done
  if [[ -z "$testfile" ]]; then
    result_lines+=("$name|${VER[$name]}|missing-test|no test file||${PORF[$name]:-}")
    echo "$name@${VER[$name]} -> missing-test (no test file)"
    continue
  fi

  # 1. node baseline
  if ! timeout 60 node "$testfile" > "$RAW/$stem.node.stdout" 2> "$RAW/$stem.node.stderr"; then
    result_lines+=("$name|${VER[$name]}|node-baseline-failed|node itself failed||${PORF[$name]:-}")
    echo "$name@${VER[$name]} -> node-baseline-failed"
    continue
  fi

  # 2. kiln run --native (from this dir so node_modules resolves)
  cd "$HERE" || exit 1
  start=$(date +%s)
  timeout "$TIMEOUT_S" "$KILN_BIN" --one-shot run --native "$testfile" \
    > "$RAW/$stem.kiln.stdout" 2> "$RAW/$stem.kiln.stderr"
  code=$?
  end=$(date +%s)
  dur=$((end - start))
  cd - >/dev/null || exit 1

  status=""; detail=""
  if ((code == 124)); then
    status="timeout"; detail="kiln run --native exceeded ${TIMEOUT_S}s"
  elif grep -qiE "not supported|cannot resolve|unsupported module|no such module|module not found" "$RAW/$stem.kiln.stderr"; then
    status="missing-api"; detail=$(grep -iE "not supported|cannot resolve|unsupported module|no such module|module not found" "$RAW/$stem.kiln.stderr" | head -2 | tr '\n' ';')
  elif ((code != 0)); then
    # distinguish compile-error from runtime-error: compile errors happen before
    # any program output and mention compile/parse/syntax in stderr
    if grep -qiE "parse|syntax|failed to compile|compilation failed" "$RAW/$stem.kiln.stderr" && [[ ! -s "$RAW/$stem.kiln.stdout" ]]; then
      status="compile-error"; detail="exit=$code; stderr: $(head -c 300 "$RAW/$stem.kiln.stderr" | tr '\n' ';')"
    else
      status="runtime-error"; detail="exit=$code; stderr: $(head -c 300 "$RAW/$stem.kiln.stderr" | tr '\n' ';')"
    fi
  elif cmp -s "$RAW/$stem.node.stdout" "$RAW/$stem.kiln.stdout"; then
    status="pass"; detail="stdout identical; dur=${dur}s"
  else
    status="wrong-output"; detail="dur=${dur}s"
  fi
  echo "$name@${VER[$name]} -> $status (${dur}s) $detail"
  result_lines+=("$name|${VER[$name]}|$status|$detail|$dur|${PORF[$name]:-}")
done

TSV="$OUT/results-$(date -u +%Y-%m-%d).tsv"
{
  echo "# kiln npm-compat results"
  echo "# generated $(date -u +%Y-%m-%dT%H:%M:%SZ) by conformance/npm-compat/run.sh"
  echo "# columns: name|version|kiln_status|detail|duration_s|porffor_status"
  for l in "${result_lines[@]-}"; do echo "$l"; done
} > "$TSV"
echo "--- wrote $TSV"
pass=$(grep -c "|pass|" "$TSV" || true)
total=${#result_lines[@]}
echo "--- $pass/$total pass"
