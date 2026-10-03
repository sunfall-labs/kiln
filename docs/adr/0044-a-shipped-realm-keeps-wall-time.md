# ADR 0044: A shipped realm keeps wall time; a measured one keeps virtual time

## Status

Accepted. Refines ADRs 0012 and 0028 for natively compiled programs, and completes the clock
half of ADR 0031's host-clock policy. Deterministic execution is unchanged where it is measured.

## Context

ADR 0012 gave the realm a virtual clock: time advances only when no ready work can make progress
before the next timer, so a timer an hour away costs no wall-clock hour and every run of a program
prints the same thing. ADR 0028 implemented that with Tokio's paused time. That is right for the
interpreter under Test262, for the differential corpus that holds the native backend to
byte-identical output, and for any run whose purpose is to be compared with another.

ADR 0031 added a build policy, `NativeRealmPolicy::host_clock`, under which the emitted program
reads `Date.now()` from the host through `HostOp::ClockNow` instead of from the realm. It stopped
there: the drive that runs the program's timers still paused Tokio time, and nothing in the
command line ever set the policy, so every binary `kiln build` produced was deterministic.

A server is not a measurement. Observed on 2026-09-02 with four client processes saturating a
compiled `node:http` service: a handler that awaited `setTimeout(resolve, 0)` answered in 13 ms at
the median while every other route answered in 0.13 ms, because paused time advances only when
the runtime is idle and a saturated server never is. A timer with a positive delay under that load
does not fire until load drops, which for a request timeout or a retry is the opposite of its
purpose, and `Date.now()` answered 0.

## Decision

### The build policy is the one clock switch, and `kiln build` turns it on

`NativeRealmPolicy::host_clock` now means the whole realm keeps the host's wall clock, not only
`Date.now()`. The daemon's `kiln build` entry points (`native_build_program`,
`native_build_graph`) build with `NativeRealmPolicy::host_clock()`; `kiln run --native`, the
shared-library builds, `scripts/native.sh` and the differential corpus keep
`NativeRealmPolicy::default()`. A program that prints the clock therefore prints wall time as a
shipped binary and virtual time under the harness, which is the split the two audiences need.
The policy already participates in the program hash and the launcher identity, so the two
builds never share an artifact.

### The launcher switches the realm before entering it

Every realm the runtime archive creates starts on the virtual clock. A launcher emitted under the
host-clock policy calls `kiln_realm_use_wall_clock(realm)` between `kiln_realm_create` and
`kiln_realm_enter`, so no timer is ever armed against a clock that later jumps. The archive
refuses the call for an entered realm. In-process tests, the interpreter and every launcher built
under the default policy never make the call, which is why an archive test can still arm an
hour-long timer without waiting an hour.

### What a wall-clock realm does differently

- Tokio time is never paused, so a timer's wait is a real wait and fires whether or not the realm
  is idle. With no other work pending, the drive sleeps until the deadline instead of jumping the
  clock to it. A timer that is already due still runs before any wait, in both modes.
- The realm's `now`, which orders timers and which `kiln_realm_now` reads, is advanced to the
  host's Unix time at every turn boundary: when the realm is entered, at the top of every drive
  iteration, and before every protocol event a connection dispatches (ADR 0046). A timer armed
  inside a callback therefore measures from the present, as Node's loop time does, and
  `Date.now()` from generated C, `Temporal.Now` through the nanosecond accessor and the
  `HostOp::ClockNow` path all agree on the same clock.
- The virtual realm is untouched: `now` moves only when a timer runs, Tokio time is paused on the
  first drive turn, and `Date.now()` starts at the epoch.
- The interpreter's explicit host-clock execution uses the same split. `Date.now()` reads
  `HostOp::ClockNow`, and each Node event-loop turn advances its run-relative monotonic scheduler
  clock from that host reading before selecting timers. Ordinary interpreter execution keeps the
  virtual clock, so a busy deterministic run never observes machine speed.

## Consequences

- A compiled server's timers fire on time under load and `Date.now()` is the wall clock. The
  mixed benchmark's `/async` route measures the runtime, not its idleness.
- The private runtime ABI gains one export (`PROVIDER_ABI_VERSION` 27) and the launcher contract
  one call (`PRIVATE_ASYNC_ABI_VERSION` 6).
- `kiln build` and `kiln run --native` of the same source produce different binaries. That was
  already implied by ADR 0031's policy being part of program identity; it is now something a
  user can observe by printing the date.
- The interpreter still has only the virtual clock. Giving it wall time is a separate decision
  with its own harness consequences, and `performance.now()` for the native backend, now that
  there is a clock worth reading, is separate work.

## Alternatives rejected

**An environment variable read by the binary.** It would let one artifact serve both audiences,
but the clock would then be decided after the program hash, the harness would have to remember
to set it, and a binary's behaviour would depend on something outside the build. The policy
already exists and already changes identity; using it is smaller and cannot be forgotten.

**Advance virtual time while the runtime is busy.** That keeps determinism in name only: how much
time passes per turn would depend on the machine, so runs would no longer compare, and timers
would still not track wall time.

**Wall time everywhere, including the harness.** The differential corpus prints clock values
(`timers.ts` prints a 25 ms interval as exactly 25) and archive tests arm hour-long timers.
Losing byte-identical comparison to fix a server is the wrong trade.

**Leave it and document it.** A timer that does not fire under load is a correctness bug in the
one environment a server exists for.
