#!/usr/bin/env bash
# Native end-to-end check of kiln:gui (ADR 0074): builds conformance/gui/native-smoke.tsx and
# runs it against the real window system. macOS only; needs a logged-in session (a window
# server), so it is not part of the gate.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname)" != "Darwin" ]]; then
	echo "gui-smoke: the native GUI backend is macOS-only; skipping" >&2
	exit 0
fi
KILN="${KILN_BIN:-target/release/kiln}"
mbx build --release --locked -p kiln-cli -p kiln-host-gui >&2
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
"$KILN" --one-shot build conformance/gui/native-smoke.tsx --output "$work/smoke" >"$work/build.log" 2>&1 || {
	cat "$work/build.log" >&2
	exit 1
}
KILN_GUI_LIB="$PWD/target/release/libkiln_host_gui.dylib" "$work/smoke" &
pid=$!
for _ in $(seq 1 120); do
	kill -0 "$pid" 2>/dev/null || break
	sleep 0.25
done
if kill -0 "$pid" 2>/dev/null; then
	kill "$pid"
	echo "gui-smoke: timed out" >&2
	exit 1
fi
wait "$pid"
