#!/usr/bin/env bash
#
# Fail if the native backend answers a member read with `undefined` where the interpreter has a
# value.
#
# WHY THIS IS A GATE AND THE REST OF THE INVENTORY IS A REPORT. A refusal stops a program at the
# member it lacked and names it. An `undefined` stops nothing: `typeof x.member === "function"`
# takes the other branch and the program dies three frames later with nothing naming the cause. A
# bundled `postcss` died on `path.dirname` that way for weeks, and a sweep on 2026-09-02 found
# thirty more -- every `node:buffer` static, `ArrayBuffer.prototype`, `Symbol.prototype`,
# `String.prototype.values`, `Array.length`. Not one was visible in a build or a test, because a
# report nobody diffs is not a gate.
#
# The build catches the half a build can: `KILN_C99_ABSENT` and `ABSENT_ANSWER_SITES` in
# `kiln-backend-c` make every place the prelude answers `undefined` a place somebody wrote down,
# with the membership predicate that proves the name absent. What a `const` cannot check is whether
# that predicate is *complete*, because completeness is a question about the other engine. This
# asks it, by running both.
#
# IT IS NOT CHEAP AND IT IS NOT PER-COMMIT. Several hundred native compiles, several gigabytes of
# `.kiln/cache`, and tens of minutes. Run it after a change to a membership list, a built-in's
# surface, or a `node:` namespace projection -- and watch the disk, because a run that ends in
# ENOSPC is not a measurement. The per-change net is `conformance/native`, whose `*-refuses.ts`
# probes and `absent-vs-unimplemented.ts` cover the decisions this sweep last made.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

KILN="${KILN_BIN:-target/release/kiln}"
if [[ ! -x $KILN ]]; then
	kiln_say "no kiln binary at $KILN; build it with \`mbx build --release -p kiln-cli\`"
	exit 1
fi

exec python3 scripts/native-surface-inventory.py --gate --kiln "$KILN" "$@"
