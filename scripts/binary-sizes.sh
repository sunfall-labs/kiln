#!/usr/bin/env bash
#
# Track native executable sizes for a fixed target set, or regenerate with --update.
#
# Byte counts are machine- and toolchain-dependent, so the baseline records the
# compiler identity alongside the sizes. A toolchain mismatch warns rather than
# fails; the regression signal is meant for runs on one pinned image (CI), with
# a threshold loose enough to absorb minor local variation.
#
# usage: scripts/binary-sizes.sh [options]
#   --check               verify current sizes against baseline (default)
#   --update              regenerate/update the committed baseline JSON
#   --baseline <path>     custom baseline JSON path (default: bench/results/binary-sizes-baseline.json)
#   --threshold <pct>     growth percentage that fails --check (default: 5)
#   --kiln-bin <path>     kiln CLI binary (default: target/release/kiln, else target/debug/kiln)
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

KILN_BIN="${KILN_BIN:-}"
BASELINE="bench/results/binary-sizes-baseline.json"
THRESHOLD="5"
UPDATE="false"

while [[ $# -gt 0 ]]; do
	case "$1" in
		--check) UPDATE="false"; shift ;;
		--update) UPDATE="true"; shift ;;
		--baseline) BASELINE="$2"; shift 2 ;;
		--threshold) THRESHOLD="$2"; shift 2 ;;
		--kiln-bin) KILN_BIN="$2"; shift 2 ;;
		-h | --help)
			sed -n '2,/^set -euo pipefail$/p' "$0" | grep '^#' | sed 's/^# \?//'
			exit 0
			;;
		*)
			echo "binary-sizes: unknown option '$1'" >&2
			exit 2
			;;
	esac
done

if [[ -z $KILN_BIN ]]; then
	if [[ -x target/release/kiln ]]; then
		KILN_BIN="target/release/kiln"
	elif [[ -x target/debug/kiln ]]; then
		KILN_BIN="target/debug/kiln"
	else
		kiln_cargo build --locked -p kiln-cli >&2
		KILN_BIN="target/debug/kiln"
	fi
fi

# name:path:extra-flags. Keep this set small and fast: native release compiles
# are the cost of every run. All paths relative to the repository root.
TARGETS=(
	"hello:examples/hello.ts:"
	"modules:examples/modules/main.ts:"
	"http-server:bench/server/serve.mjs:--node"
)

WORK="$(mktemp -d "${TMPDIR:-/tmp}/kiln-binary-sizes.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

CLANG_VERSION="$(clang --version 2>/dev/null | head -n 1 || echo unknown)"
PLATFORM="$(uname -s -m)"

measure() {
	local name="$1" path="$2" flags="$3"
	local out="$WORK/$name"
	# shellcheck disable=SC2086
	"$KILN_BIN" --one-shot build "$path" --profile native-release $flags --output "$out" >/dev/null 2>&1
	if [[ ! -x $out ]]; then
		echo "binary-sizes: build failed for $name ($path)" >&2
		return 1
	fi
	if stat -c%s /dev/null >/dev/null 2>&1; then
		stat -c%s "$out"
	else
		stat -f%z "$out"
	fi
}

MEASURED="{}"
for spec in "${TARGETS[@]}"; do
	name="${spec%%:*}"
	rest="${spec#*:}"
	path="${rest%%:*}"
	flags="${rest#*:}"
	bytes="$(measure "$name" "$path" "$flags")"
	MEASURED="$(python3 - "$MEASURED" "$name" "$bytes" <<-'PY'
		import json, sys
		data = json.loads(sys.argv[1])
		data[sys.argv[2]] = int(sys.argv[3])
		print(json.dumps(data))
		PY
	)"
done

NEW_BASELINE="$(python3 - "$MEASURED" "$CLANG_VERSION" "$PLATFORM" <<-'PY'
	import json, sys
	print(json.dumps({
	    "version": 1,
	    "toolchain": sys.argv[2],
	    "platform": sys.argv[3],
	    "targets": {
	        "hello": {"path": "examples/hello.ts", "node": False},
	        "modules": {"path": "examples/modules/main.ts", "node": False},
	        "http-server": {"path": "bench/server/serve.mjs", "node": True},
	    },
	    "bytes": json.loads(sys.argv[1]),
	}, indent=2, sort_keys=True))
	PY
)"

if [[ $UPDATE == "true" ]]; then
	printf '%s\n' "$NEW_BASELINE" >"$BASELINE"
	kiln_say "binary-sizes: wrote $BASELINE"
	python3 - "$NEW_BASELINE" <<-'PY'
		import json, sys
		for name, size in sorted(json.loads(sys.argv[1])["bytes"].items()):
		    print(f"  {name}: {size} bytes")
		PY
	exit 0
fi

if [[ ! -f $BASELINE ]]; then
	echo "binary-sizes: no baseline at $BASELINE; run with --update first" >&2
	exit 1
fi

python3 - "$BASELINE" "$NEW_BASELINE" "$THRESHOLD" <<-'PY'
	import json, sys

	with open(sys.argv[1], encoding="utf-8") as handle:
	    baseline = json.load(handle)
	new = json.loads(sys.argv[2])
	threshold = float(sys.argv[3])

	if baseline.get("toolchain") != new.get("toolchain"):
	    print(f"warning: toolchain differs (baseline: {baseline.get('toolchain')}; "
	          f"current: {new.get('toolchain')}). Comparing anyway.")
	if baseline.get("platform") != new.get("platform"):
	    print(f"warning: platform differs (baseline: {baseline.get('platform')}; "
	          f"current: {new.get('platform')}). Comparing anyway.")

	old_bytes = baseline.get("bytes", {})
	new_bytes = new.get("bytes", {})
	if set(old_bytes) != set(new_bytes):
	    print(f"FAILED: target set changed (baseline: {sorted(old_bytes)}; "
	          f"current: {sorted(new_bytes)}). Update the baseline explicitly.")
	    sys.exit(1)

	failures = 0
	for name in sorted(new_bytes):
	    old, new_size = old_bytes[name], new_bytes[name]
	    delta = (new_size - old) / old * 100 if old else 0.0
	    status = "ok"
	    if delta > threshold:
	        status = "REGRESSION"
	        failures += 1
	    print(f"  {name}: {old} -> {new_size} bytes ({delta:+.1f}%) {status}")
	if failures:
	    print(f"FAILED: {failures} target(s) grew more than {threshold}%")
	    sys.exit(1)
	print("binary-sizes: within threshold")
	PY
