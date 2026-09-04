#!/usr/bin/env bash
#
# Fetch the pinned Test262 commit into a durable per-machine cache.
#
# Once per machine, not once per worktree: 53,874 files multiplied by every checkout is exactly the
# cost the worktree habit in AGENTS.md exists to avoid, which is also why this is a cache rather
# than a submodule. The pin is enforced here and again by `scripts/test262.sh`, so a shared
# checkout gives up no guarantee.
#
# Deliberately not under `TMPDIR`. macOS's periodic cleaner empties that on a day boundary and took
# a whole checkout mid-session once -- and an emptied corpus reports `pass 0 fail 0` and exits 0,
# which reads like a clean run rather than a missing one.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

readonly TEST262_REPOSITORY="https://github.com/tc39/test262.git"
readonly TEST262_COMMIT="771005236e88a909635104e03ba12559688c0172"
CHECKOUT="${KILN_TEST262_CHECKOUT:-$HOME/.cache/kiln/test262-${TEST262_COMMIT:0:10}}"

die() {
	echo "test262-fetch.sh: $*" >&2
	exit 1
}

if [[ -d $CHECKOUT/.git ]]; then
	head="$(git -C "$CHECKOUT" rev-parse HEAD 2>/dev/null || echo none)"
	files="$(find "$CHECKOUT/test/built-ins" -name '*.js' -print 2>/dev/null | wc -l | tr -d ' ')"
	if [[ $head == "$TEST262_COMMIT" && $files -gt 1000 ]]; then
		echo "test262: already at $TEST262_COMMIT ($files files under built-ins)"
		echo "         $CHECKOUT"
		exit 0
	fi
	echo "test262: checkout is stale or emptied (head=$head, $files files); refetching" >&2
	rm -rf "$CHECKOUT"
fi

mkdir -p "$(dirname -- "$CHECKOUT")"
rm -rf "$CHECKOUT"
mkdir -p "$CHECKOUT"
git -C "$CHECKOUT" init --quiet .
git -C "$CHECKOUT" remote add origin "$TEST262_REPOSITORY"
git -C "$CHECKOUT" fetch --quiet --depth=1 origin "$TEST262_COMMIT" ||
	die "could not fetch $TEST262_COMMIT from $TEST262_REPOSITORY"
git -C "$CHECKOUT" checkout --quiet FETCH_HEAD

head="$(git -C "$CHECKOUT" rev-parse HEAD)"
[[ $head == "$TEST262_COMMIT" ]] || die "checked out $head, expected $TEST262_COMMIT"
files="$(find "$CHECKOUT/test/built-ins" -name '*.js' -print | wc -l | tr -d ' ')"
[[ $files -gt 1000 ]] || die "checkout looks empty ($files files under built-ins)"

echo "test262: fetched $TEST262_COMMIT ($files files under built-ins)"
echo "         $CHECKOUT"
