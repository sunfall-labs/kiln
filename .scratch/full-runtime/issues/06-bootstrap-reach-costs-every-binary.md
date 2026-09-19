# Issue 06: the runtime bootstrap's reach is paid by every binary

Status: ready
Feature: full-runtime
Governing ADR: ADR 0063 rule 4, ADR 0062

Measured 2026-09-19 in a clean cache: a `console.log("hi")` binary is 1.83 MB at `740f4fb4` and
2.59 MB after the bootstrap gained the console methods, the Set algebra, the Error and
symbol-keyed definitions and the Date spellings. The bootstrap object itself grew 68 KB
(385 KB, emitted generic-only; 0.9 MB more than that with clones, which is why it is generic-only
now). The rest is *reach*: the new code references runtime paths -- sort, spread, generators,
`defineProperty`, `Symbol.for`, `padStart` -- that the link used to dead-strip from a program
that never touched them, and `hello` now carries them for a `console.table` it never calls.

`URL` shows the shape of the answer: it rides `KILN_RETAIN_GROUP_URL`, so a program that names
neither it nor anything built on it gets no class and no crate, and `hello` is unchanged by it.
The Set algebra, the Date spellings and the symbol-keyed prototype methods can ride the groups
that already exist (`DATE_SETTER` for dates) or new ones keyed on the builtin the liveness
analysis already records; the console methods are the one section every program keeps.

Done when `hello` in a clean cache is within 100 KB of 1.83 MB and every probe in
`conformance/native` still matches.
