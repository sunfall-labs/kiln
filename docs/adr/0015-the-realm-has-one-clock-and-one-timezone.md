# ADR 0015: The realm has one clock and one timezone, and neither is the host's

## Status

Accepted.

## Context

ADR 0012 put a virtual clock on the machine so timers are deterministic, and closed by noting that
`Date` had not arrived yet and that when it did it would have to decide whether it read that clock
or the real one. This is that decision, plus the timezone question it drags along.

Both are the same shape of problem. A program that asks the host what time it is, or what zone it
is in, produces a different answer on a different machine and at a different moment. That is fine
for an application and corrosive for a compiler: it means a build is not reproducible, a test is
not repeatable, and a cached artifact may have been produced under conditions that no longer hold.
It also breaks the differential harness outright, since `node` reads the real clock and the
ambient `TZ` and kiln cannot agree with either by construction.

## Decision

**`Date.now()` and `new Date()` read the virtual clock**, the same one timers advance. A program
that asks the time twice with no timer in between gets the same answer, because no time has
passed: nothing has happened. The clock starts at the epoch, so an unqualified `new Date()` is
1970-01-01 — which is obviously not the real time, and is meant to be. A plausible-looking wrong
answer would be worse than an implausible one.

**The realm's timezone is UTC**, and `getTimezoneOffset()` returns 0 rather than leaving that to be
discovered. The local readers — `getFullYear`, `getMonth`, `getHours` and the rest — are the UTC
readers. This is a defined and discoverable property of the realm, not a silent substitution: a
program can ask and gets a consistent answer, and on a host already in UTC the behaviour is
identical to node's.

**Parsing accepts ISO 8601 and nothing else.** The specification permits an implementation-defined
set of further formats, and every one of them turns some typo into a date rather than a `NaN`.

`scripts/differential.sh` runs node under `TZ=UTC` so the two implementations are in the same zone.
It cannot pin the *clock* the same way, so a probe must not print the current instant; the script
says so, and every date probe writes its instants out explicitly.

## Consequences

Everything downstream of a stated instant is comparable against node and is checked that way:
`conformance/differential/dates.js` covers pre-epoch instants, leap days, the century rule, field
rollover, invalid dates propagating through every reader, and the arithmetic that makes a
difference of two dates a duration.

Two things a program can legitimately want are now impossible, and both are deliberate. It cannot
measure elapsed wall-clock time — `Date.now()` differences are always zero outside a timer — and it
cannot render a local time. The first is what a benchmark harness wants, and `kiln-bench` measures
from Rust where the real clock is available. The second is what a user-facing program wants, and
when it becomes necessary the honest form is an explicit zone argument rather than an ambient one:
a program that says which zone it means stays reproducible, and one that asks the host does not.

The virtual clock also makes a whole class of test possible that a real clock makes flaky, which is
the same argument ADR 0012 made for timers and is worth restating: a race that depends on the order
two callbacks run in is decided by the program here, so it reproduces exactly.

Nothing here reads the host clock at all, so there is no hidden dependency to remove later if the
decision is revisited — only one to add, in one place, deliberately.
