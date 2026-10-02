#!/usr/bin/env bash
# Runs the node conformance corpus N times and reports how often each test failed.
set -u
n="${1:-30}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root" || exit 1
out="$(mktemp)"
for _ in $(seq 1 "$n"); do
  ./target/debug/kiln test conformance/node --node 2>&1 | grep -E '^\s+FAIL\s+node-' >>"$out"
done
echo "runs: $n"
echo "failures by test:"
if [ -s "$out" ]; then
  awk '{print $2}' "$out" | sort | uniq -c | sort -rn
else
  echo "  (none)"
fi
echo "total failing test instances: $(wc -l <"$out")"
rm -f "$out"
