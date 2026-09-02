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
# WHY IT RUNS EACH BUNDLE TWICE. Kiln has two engines, and for a long time this script exercised
# only the interpreter. That is how the native backend came to be missing ten `Number` statics and
# eight well-known symbols without anything noticing: `Number.MAX_VALUE` is the first property a
# bundled `zod` reads, so its absence refused the whole module, and no harness was asking. The
# native pass closes that. It is the same bundles and the same expected bytes -- the point is that
# a library is a far better interrogator of the builtin surface than any probe we would write.
#
# The two passes judge differently, on purpose. Interpreted, there is no acceptable outcome but
# node's bytes. Natively, a refusal is the honest state of a backend that is still being filled
# in, and it names what it lacks; it is reported and does not fail. A *divergence* fails in both,
# because a wrong answer is worse than an absent one -- a refusal stops the program where a wrong
# answer lets it continue on a bad value. Set KILN_THIRDPARTY_NATIVE=0 to skip the native pass
# when you only want the fast half.
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

# The four probes build.sh bundles as CommonJS for the *node* platform, because the libraries
# behind them reach `path`, `fs`, `os` and `util` and cannot be bundled for the neutral one at
# all. Running one of those without the Node Adapter is a harness bug rather than a finding: it
# reports "require is not defined", which says nothing about the library. Kept in step with the
# NODE_PLATFORM list in conformance/thirdparty/build.sh, which is where the decision is made.
NODE_PLATFORM=" typescript babel postcss react-ssr "

matched=0
missing=0
diverged=0
DIVERGENCES=()

for probe in "$BUNDLES"/probes/*.mjs; do
	name="$(basename "$probe" .mjs)"
	bundle="$BUNDLES/$name.bundle.js"
	if [[ ! -f $bundle ]]; then
		missing=$((missing + 1))
		continue
	fi
	adapter=""
	if [[ $NODE_PLATFORM == *" $name "* ]]; then
		adapter="--node"
	fi
	# Same TZ discipline as the differential harness: kiln's realm is always UTC (ADR 0015) and
	# node takes its zone from the environment, so the two are only comparable under TZ=UTC.
	expected="$(TZ=UTC node "$bundle" 2>&1 || true)"
	actual="$("$KILN" --one-shot ${adapter:+"$adapter"} run "$bundle" 2>&1 || true)"
	if [[ $actual == "$expected" ]]; then
		matched=$((matched + 1))
	else
		diverged=$((diverged + 1))
		DIVERGENCES+=("  $name")
		DIVERGENCES+=("    node: $(printf '%s' "$expected" | head -3 | tr '\n' '|')")
		DIVERGENCES+=("    kiln: $(printf '%s' "$actual" | head -3 | tr '\n' '|')")
	fi
done

kiln_say "thirdparty: $matched matched, $diverged diverged, $missing not built"

if ((missing > 0)); then
	kiln_say "  ($missing bundle(s) absent; run conformance/thirdparty/build.sh to produce them)"
fi

# Reported here, but not exited on yet: the native pass below must run even when the interpreted
# one has found something. Gating it behind a green interpreted run would rebuild the very hole
# this script was widened to close -- a single known divergence would silently stop anyone from
# ever seeing the native half. The exit status is decided once, at the end, on both passes.
if ((${#DIVERGENCES[@]} > 0)); then
	kiln_say ""
	kiln_say "DIVERGED - a real library got a different answer here than under node:"
	printf '%s\n' "${DIVERGENCES[@]}" | while IFS= read -r line; do kiln_say "$line"; done
fi

if [[ ${KILN_THIRDPARTY_NATIVE:-1} == 0 ]]; then
	((diverged > 0)) && exit 1
	exit 0
fi

native_matched=0
native_refused=0
native_diverged=0
REFUSALS=()
NATIVE_DIVERGENCES=()

for probe in "$BUNDLES"/probes/*.mjs; do
	name="$(basename "$probe" .mjs)"
	bundle="$BUNDLES/$name.bundle.js"
	if [[ ! -f $bundle ]]; then
		continue
	fi
	adapter=""
	if [[ $NODE_PLATFORM == *" $name "* ]]; then
		adapter="--node"
	fi
	expected="$(TZ=UTC node "$bundle" 2>&1 || true)"
	actual="$("$KILN" --one-shot ${adapter:+"$adapter"} run --native "$bundle" 2>&1 || true)"
	if [[ $actual == "$expected" ]]; then
		native_matched=$((native_matched + 1))
	# A refusal names itself, and the name is the whole value of the outcome: it says which member
	# of which builtin the backend has not been given yet, which is the next thing to implement.
	# Matching on the marker rather than on the exit status keeps a library that legitimately
	# prints the word "refusal" from being miscounted.
	elif [[ $actual == *"native backend refusal"* || $actual == *"unsupported native emission:"* ]]; then
		native_refused=$((native_refused + 1))
		REFUSALS+=("  $(printf '%-14s %s' "$name" "$(printf '%s' "$actual" | grep -oE '(operation=[^ ]+( detail=[^=]*)?|unsupported native emission: .*)' | head -1)")")
	else
		native_diverged=$((native_diverged + 1))
		NATIVE_DIVERGENCES+=("  $name")
		NATIVE_DIVERGENCES+=("    node:   $(printf '%s' "$expected" | head -3 | tr '\n' '|')")
		NATIVE_DIVERGENCES+=("    native: $(printf '%s' "$actual" | head -3 | tr '\n' '|')")
	fi
done

kiln_say "thirdparty (native): $native_matched matched, $native_refused refused, $native_diverged diverged"

if ((native_refused > 0)); then
	kiln_say ""
	kiln_say "Refused by the native backend (the backlog, not a failure):"
	printf '%s\n' "${REFUSALS[@]}" | while IFS= read -r line; do kiln_say "$line"; done
fi

if ((native_diverged > 0)); then
	kiln_say ""
	kiln_say "DIVERGED natively - the compiled program answered, and answered wrongly:"
	printf '%s\n' "${NATIVE_DIVERGENCES[@]}" | while IFS= read -r line; do kiln_say "$line"; done
fi

if ((diverged > 0 || native_diverged > 0)); then
	exit 1
fi
