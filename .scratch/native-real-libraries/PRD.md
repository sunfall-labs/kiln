# Real libraries run natively

Status: in-progress

## Why

Native compilation is the point of this engine, and until 2026-09-01 nothing measured it against
real code. `scripts/thirdparty.sh` ran nineteen bundled npm libraries against the interpreter only,
so the native backend's builtin surface could drift arbitrarily far from the interpreter's without
any harness objecting. It had: `Number` published fifteen statics interpreted and five natively,
and `Symbol` thirteen well-known symbols against five. `Number.MAX_VALUE` is the first property a
bundled `zod` reads, so a two-line gap refused an entire module, and we found it only because
someone went looking bundle by bundle.

The script now runs both engines (`5359948b`). The first native run of the corpus is the baseline
below.

## Baseline, 2026-09-01, at `5359948b`

    thirdparty (native): 8 matched, 10 refused, 1 diverged

Eight of nineteen real libraries compile and run natively, byte-identical to node. `chalk` and
`semver` joined that set the same day, when the eight missing well-known symbols landed.

## How to read the outcomes

A **refusal** names a member the backend has not been given yet and stops. It is honest backlog and
does not fail the script. A **divergence** means the compiled program answered, and answered
wrongly; it fails, because a refusal stops a program where a wrong answer lets it continue on a bad
value. This is the same taxonomy `scripts/native.sh` uses.

## Shape of the backlog

Five of the ten refusals are one architectural problem, not five bugs: the builtin surface is
written down twice, once in the interpreter's Rust and once as hand-written `same_name` chains in
the emitted C, with nothing checking that the two agree. A sweep on 2026-09-01 measured the drift
at **55 of 80 builtins**. That is being fixed separately as one table with two emitters; these
issues record what real libraries hit, so the fix can be checked against actual demand rather than
against a list someone wrote from memory.

## The backlog is no longer discovered one bundle at a time

ADR 0043 landed the one-table fix, and it covers the fifteen table-driven builtins. Everything
else --- the prototype method tables, the `node:` namespaces, the globals that are not
table-driven --- had no inventory until `scripts/native-surface-inventory.py`, which asks both
engines about every member of every receiver and writes `native-surface-inventory.md` beside this
file. At `1f1ea59f` that is **57 diverged, 305 refused, 638 implemented** out of 1000 members,
plus 51 names published natively that the interpreter does not have.

Read the report before opening an issue here, and read issue 08 before reading the report. The
next gap a bundle finds is almost certainly already a row in it, with the class it belongs to and
how many others share its cause --- and two of its columns need the caveat issue 08 gives them:
a `node:` member counted *implemented* may only agree on `typeof` and still refuse when called,
and 124 of the 362 missing members are `length`, `name`, `constructor` and `prototype`, which is
function-object metadata rather than backlog a real program will hit.
