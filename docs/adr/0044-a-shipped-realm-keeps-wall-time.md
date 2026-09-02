# ADR 0044: A shipped realm keeps wall time; a measured one keeps virtual time

## Status

Accepted. Refines ADRs 0012, 0015 and 0031 for natively compiled programs run outside the
conformance harness. Deterministic execution is unchanged where it is measured.

## Context

ADR 0012 gave the realm a virtual clock: time advances only when no ready work can make progress
before the next timer, so a timer an hour away costs no wall-clock hour and every run of a program
prints the same thing. ADR 0028 implemented that with Tokio's paused time. That is right for the
interpreter under Test262, for the differential corpus that holds the native backend to
byte-identical output, and for any run whose purpose is to be compared with another.

A server is not that. Measured on 2026-09-02 with four client processes saturating a compiled
`node:http` service, a handler that awaited `setTimeout(resolve, 0)` answered in 13 ms at the
median while every other route answered in 0.13 ms. Paused Tokio time advances only when the
runtime is idle, and a saturated server is never idle. A timer with a positive delay under the
same load does not fire until load drops, which for a request timeout or a retry is the opposite of
its purpose. `Date.now()` answers 0 in such a binary, `performance.now()` is not emitted, and
`setTimeout(0)` fires "at the next turn" of a clock that is not moving.

The two needs do not conflict; they are two configurations of one realm, and the question is only
which one a given process gets by default.

## Decision

### The clock is chosen per process, and the default follows the purpose

A native binary run by the conformance harness, by `scripts/native.sh`, or by `kiln run --native`
keeps the virtual clock: the runner passes `KILN_CLOCK=virtual` to the child. Every other native
process, which is to say a binary a user built and started, keeps wall time. The interpreter is
unchanged in both modes. An explicit `KILN_CLOCK=virtual` or `KILN_CLOCK=live` in the environment
of any native process wins.

### Wall time means three things

- Timers wait on the live Tokio clock. Time is not paused, so a sleep fires when its deadline
  passes whether or not the runtime is idle. A timer that is already due runs before the drive
  waits at all (that part holds in both modes: a due timer is not a wait case).
- The realm's virtual `now`, which orders timers and which `Date.now()`, `Date`, `performance`
  and Temporal's `Now` read, advances to the monotonic time elapsed since the realm started at
  every turn boundary, and is anchored to the host's Unix time at start. `Date.now()` therefore
  answers the wall clock, as Node does, and timer ordering by `(deadline, sequence)` is unchanged.
- The realm's time zone stays what ADR 0015 says (UTC) until ADR 0031's successor supplies the
  host zone as a capability. Wall time is a clock decision, not a locale decision.

### Virtual time keeps its contract

With `KILN_CLOCK=virtual` nothing above applies: Tokio time is paused, `now` advances only when a
timer runs, `Date.now()` starts at the epoch, and the differential corpus continues to compare two
engines that agree on every printed instant. The runner sets it, so a contributor running
`scripts/native.sh` sees no change.

## Consequences

- A compiled server's timers fire on time under load, and `Date.now()` is the wall clock. The
  mixed benchmark's `/async` route stops being a measurement of idleness.
- A program whose output depends on the clock prints different things under `kiln run --native`
  and as a shipped binary. That was already true of Node versus the interpreter; the harness
  documents which mode it is measuring, and `KILN_CLOCK` makes either reproducible on demand.
- The interpreter still has only the virtual clock. Giving it wall time is a separate decision
  with its own harness consequences.
- `performance.now()` for the native backend is unblocked by having a clock worth reading; it is
  not implemented by this ADR.

## Alternatives rejected

**Advance virtual time when the runtime is busy.** That keeps determinism in name only: how much
time passes per turn would depend on the machine, so runs would no longer compare, and timers
would still not track wall time.

**Wall time everywhere, including the harness.** The differential corpus prints clock values
(`timers.ts` prints a 25 ms interval as exactly 25) and Test262 expects a paused clock in places.
Losing byte-identical comparison to fix a server is the wrong trade.

**Leave it and document it.** A timer that does not fire under load is a correctness bug in the
one environment a server exists for.
