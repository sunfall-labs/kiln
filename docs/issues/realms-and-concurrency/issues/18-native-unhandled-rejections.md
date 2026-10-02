# 18. The native backend reports unhandled rejections

Status: resolved

Found by issue 14: a natively compiled program with an unhandled promise rejection exits 0 with no
message, where node prints the error and exits 1 (and the interpreter now matches node). That is a
silent wrong answer of the most expensive kind (`CLAUDE.md`). Neither engine supports
`process.on("unhandledRejection")` / `"rejectionHandled"` yet.

Implement node's unhandled-rejection tracking in the realm (the realm decides observable order, ADR
0028), shared by both engines: detection at the microtask checkpoint as node does, the default
`--unhandled-rejections=throw` behaviour (print, exit 1), and the `process` events. Record node's
output in `conformance/regress` cases (stdout and exit code).

## Acceptance

- Native and interpreter both match node's stdout, stderr summary and exit code for an unhandled
  rejection, a late-handled one, and with `process.on("unhandledRejection")` installed.

## Comments

**2026-09-28 — scheduled realm exit for a rejected startup module.**

A startup module whose top-level await rejected beside a live timer previously called `exit(1)`
from its promise reaction. On a scheduled lane that could abort the whole process during Rust
thread-local teardown. The generated graph now stops the promise checkpoint with a private
non-throw status and lets the realm launcher report the original rejection and destroy the
realm. The live-handle fixture checks exit code 1 and the error text for a standalone realm,
two dedicated realms, and two scheduled realms. The routine gate passed all 10 steps.

**2026-09-27 — branch `work/node-parity-rejections`: resolved.**

Node's `processPromiseRejections` is now one implementation, `kiln-builtins/src/rejections.ts`,
in the common bootstrap of both engines. Each engine keeps only what needs its own promise
representation: [[PromiseIsHandled]] (a new `handled` byte on the native `kiln_promise`, in the
padding before `value`), HostPromiseRejectionTracker's "reject" (a promise rejected with nothing
attached is queued, state `Waiting`) and "handle" (a waiting one is simply no longer reported --
the common `await`/`catch` path never reaches the TypeScript; a reported one is queued as a late
handle). At the end of every microtask checkpoint, once the tick and promise queues are both
empty, the engine hands the two lists to the function the bootstrap registered through a new
primitive, `__kilnPromiseRejections` (interpreter `Native::PromiseRejectionsRaw`, native method
id **536**), then drains what that queued and asks again -- node's `processTicksAndRejections`
loop. Natively this is the promise lane itself (`kiln_c99_run_promise_jobs` / `..._pending`),
which the realm's drain already calls after the ticks and before a host completion, so the
provider ABI did not change.

The TypeScript side is node's default `--unhandled-rejections=throw`: `unhandledRejection`
`(reason, promise)` suppresses the default; without it an `uncaughtException` listener gets the
error with origin `"unhandledRejection"`, and otherwise the reason (or node's
`UnhandledPromiseRejection`, `ERR_UNHANDLED_REJECTION`, naming a non-error reason as V8's
`NoSideEffectsToString` does) is thrown out of the checkpoint, so each engine reports it as an
uncaught exception and exits 1. A reported promise handled later is `rejectionHandled`, or node's
`PromiseRejectionHandledWarning` (a `warning` event from a tick, written to stderr) with no
listener. `__kilnJobsPending`'s bit 4 is node's `hasRejectionToWarn` in both engines. A realm
without the bootstrap still fails on its first rejection, and the interpreter keeps the
`(in promise)` prefix Test262's runner reads.

- Regress, recorded against node 26.8.1, both engines matching stdout and exit status:
  `r20-unhandled-rejection-exits` (exit 1, `Error: boom`), `r21-unhandled-rejection-non-error`
  (exit 1, `UnhandledPromiseRejection: ... "42".`), `r22-unhandled-rejection-events`,
  `r23-rejection-handled-late`, `r24-unhandled-rejection-uncaught-exception`. The runner gained
  `expected_uncaught` (the text node's stderr summary line must appear in the uncaught report;
  the program must end that way), since `expected_stdout` alone could not hold a crash.
- Native corpus: `conformance/native/unhandled-rejection-events.cjs`.
- Not done: `--unhandled-rejections` modes other than `throw` (no way to pass node flags yet);
  `uncaughtExceptionMonitor`. The stderr format of an uncaught exception is each engine's
  existing one (the summary line is node's; node's source excerpt and stack are not).
