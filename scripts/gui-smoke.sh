#!/usr/bin/env bash
# Native end-to-end check of kiln:gui (ADR 0074): builds conformance/gui/native-smoke.tsx and
# runs it against the real window system. It needs one, so it is not part of the gate:
#
#   macOS  a logged-in session (a window server); the AppKit backend.
#   Linux  a display (DISPLAY or WAYLAND_DISPLAY) and the GTK 4 development files; the GTK
#          backend. Headless: `xvfb-run -a scripts/gui-smoke.sh`.
#
# Without a display the script says so and exits 0.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"
case "$(uname)" in
Darwin) ;;
Linux)
	if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
		echo "gui-smoke: no display (DISPLAY or WAYLAND_DISPLAY); try xvfb-run -a $0; skipping" >&2
		exit 0
	fi
	;;
*)
	echo "gui-smoke: kiln:gui has no widget backend on $(uname); skipping" >&2
	exit 0
	;;
esac
target="${CARGO_TARGET_DIR:-$PWD/target}"
KILN="${KILN_BIN:-$target/release/kiln}"
kiln_cargo build --release --locked -p kiln-cli -p kiln-host-gui >&2
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
"$KILN" --one-shot build conformance/gui/native-smoke.tsx --output "$work/smoke" >"$work/build.log" 2>&1 || {
	cat "$work/build.log" >&2
	exit 1
}
# The shipping path: the bridge linked into the binary, the realm parked in the run loop. Set
# KILN_GUI_LIB to a development library (target/release/libkiln_host_gui.dylib or .so) to
# exercise the polling fallback instead.
"$work/smoke" &
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
