#!/usr/bin/env bash
# Check (or with --update, regenerate) the committed Web Platform Tests chunk baseline: the
# WinterTC Minimum Common API's own conformance suite, through both engines. See scripts/wpt.py.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"
UPDATE=""
[[ ${1:-} == --update ]] && UPDATE=--update
KILN_BIN="${KILN_WPT_KILN:-target/release/kiln}"
[[ -x $KILN_BIN ]] || { echo "wpt.sh: no kiln binary at $KILN_BIN" >&2; exit 1; }
ARGS=(baseline --kiln "$KILN_BIN" --baseline conformance/wpt-baseline.json --jobs "${KILN_WPT_JOBS:-4}")
[[ -n ${KILN_WPT_REPORT:-} ]] && ARGS+=(--report "$KILN_WPT_REPORT")
[[ -n ${KILN_WPT_CHECKOUT:-} ]] && ARGS+=(--checkout "$KILN_WPT_CHECKOUT")
[[ -n $UPDATE ]] && ARGS+=(--update)
python3 "$PWD/scripts/wpt.py" "${ARGS[@]}"
