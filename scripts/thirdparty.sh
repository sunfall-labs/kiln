#!/usr/bin/env bash
#
# Run the third-party bundles in conformance/thirdparty against node, and fail on any divergence.
#
# WHY THIS EXISTS ALONGSIDE scripts/differential.sh. That one runs probes *we* wrote, so it can
# only find what someone thought to ask about. These are real libraries -- date-fns, immer, zod,
# semver, mustache, js-yaml -- written against the whole language rather than against what kiln
# happens to implement, and every one of them found something within seconds of first running.
# None of those was a refusal; each was a plausible answer that was wrong. Writing a string
# index, a Date setter, `Map.values()`, `array.length = 0`, and `===` on a date, are things a
# hand-written probe reaches only if you already suspect them.
#
# There is no "refused" outcome here. A library either produces node's bytes or it does not: the
# programs are far too large for a refusal to be a useful signal, and any of them stopping early
# is a failure whatever the reason.
#
# Bundles small enough to read are committed; the large ones are gitignored, and this skips them
# with a note rather than failing, because a checkout that has not run build.sh is not broken.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

BUNDLES="${KILN_THIRDPARTY_DIR:-conformance/thirdparty}"

if ! command -v node >/dev/null 2>&1; then
	kiln_say "SKIP: node is not on PATH, so there is nothing to compare against."
	exit 0
fi

KILN="${KILN_BIN:-}"
if [[ -z $KILN ]]; then
	kiln_cargo build --locked -p kiln-cli >&2
	KILN="$(kiln_repo_root)/target/debug/kiln"
fi

matched=0
missing=0
DIVERGENCES=()

for probe in "$BUNDLES"/probes/*.mjs; do
	name="$(basename "$probe" .mjs)"
	bundle="$BUNDLES/$name.bundle.js"
	if [[ ! -f $bundle ]]; then
		missing=$((missing + 1))
		continue
	fi
	# Same TZ discipline as the differential harness: kiln's realm is always UTC (ADR 0015) and
	# node takes its zone from the environment, so the two are only comparable under TZ=UTC.
	expected="$(TZ=UTC node "$bundle" 2>&1 || true)"
	actual="$("$KILN" --one-shot run "$bundle" 2>&1 || true)"
	if [[ $actual == "$expected" ]]; then
		matched=$((matched + 1))
	else
		DIVERGENCES+=("  $name")
		DIVERGENCES+=("    node: $(printf '%s' "$expected" | head -3 | tr '\n' '|')")
		DIVERGENCES+=("    kiln: $(printf '%s' "$actual" | head -3 | tr '\n' '|')")
	fi
done

kiln_say "thirdparty: $matched matched, ${#DIVERGENCES[@]} diverged, $missing not built"

if ((missing > 0)); then
	kiln_say "  ($missing bundle(s) absent; run conformance/thirdparty/build.sh to produce them)"
fi

if ((${#DIVERGENCES[@]} > 0)); then
	kiln_say ""
	kiln_say "DIVERGED - a real library got a different answer here than under node:"
	printf '%s\n' "${DIVERGENCES[@]}" | while IFS= read -r line; do kiln_say "$line"; done
	exit 1
fi
