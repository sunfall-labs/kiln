# 04: An idle wall-clock server must dump within a bounded time

Status: done

## What

A listening server with nothing to do parks in `wait_for_turn(signal, None)` and stays there until
a connection arrives. With a flag-at-the-turn-boundary dump that means an idle server answers
`SIGUSR1` only when it stops being idle — the exact inversion of what the feature is for, since
the process you most want to interrogate is the one that has gone quiet.

## How

In `drive_async` (`crates/native/kiln-native-provider-ffi/src/lib.rs`), cap the park delay at 200 ms when
both hold:

- the realm keeps the host's wall clock (`NativeClock::Wall`), and
- the introspect hook reported `armed` on a previous turn (cached in a `Cell<bool>` on
  `NativeRealm`; the heap exists, and so the hook has run, well before the first park).

A `DriveWake::Timer` with no real deadline finds `run_next_timer()` empty and simply loops, which
is where the flag is checked.

## What must not be capped

A virtual-clock realm. Its park costs no wall time by construction (ADR 0012, ADR 0028) and its
whole point is that a one-hour timer costs no hour; capping it would make deterministic execution
depend on real time and would show up as flake in the conformance corpora rather than as a wrong
number here.

## Cost

Five wakeups a second on an idle armed wall-clock realm. Measured against the alternative — a
self-pipe the handler writes and the reactor selects on — this is a few lines instead of a new fd,
a new registration and a new failure mode, for a process that is by hypothesis doing nothing.

## Done when

A compiled `bench/server/serve.mjs` with no traffic dumps within a fifth of a second of
`kill -USR1`, and the conformance corpora are unchanged.
