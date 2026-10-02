# 23. `process.on(signal)` in realms, for graceful shutdown

Status: resolved

Found by issue 07: realms cannot listen for `process.on('SIGTERM')` (or SIGINT, SIGHUP, SIGUSR2,
SIGBREAK), so a server cannot drain in-flight requests on shutdown in either `KILN_WORKERS` mode or
a single realm; the process simply dies of the signal. Implement node's signal events: listeners
installed per realm, a signal with listeners no longer terminates (node's rule), delivery through
the realm's event loop in node's order; in `KILN_WORKERS` modes the supervisor (process mode) and
the signal thread (thread mode) forward to every realm, and the SIGKILL grace escalation still
applies. Also consider `process.exit` in thread mode: flush and end cleanly while other realm
threads run (today it calls C `exit`).

## Acceptance

- A server with `process.on('SIGTERM', () => server.close(...))` drains in-flight requests and exits
  0 in single-realm, process and thread modes; regress cases recorded from node for the event order.

## Comments

**2026-09-27 -- branch `work/signal-events`: resolved.** No provider ABI bump (still 48 / 139
exports) and no new method ids: everything goes through the existing `__kilnWorkerOp` /
`kiln_worker_op` and `__kilnPortWait`, with new op numbers 16-20.

- **Runtime (`kiln_runtime::signals`, new).** Process-wide: a realm *subscribes* to a signal and
  gets an endpoint of the cross-realm ports; each arrival queues one `kind::SIGNAL` message on
  every subscribed endpoint. The first subscription installs a handler (async-signal-safe:
  a byte to a self-pipe, then any replaced handler that was a function, so the SIGUSR1
  introspection dump survives a listener) and keeps the replaced action; the last one restores
  it. A `kiln-signals` thread reads the pipe. `SIGKILL`/`SIGSTOP` (and non-signals) answer
  `EINVAL`, which the TypeScript throws as node's `uv_signal_start EINVAL`. Signal names cross as
  an index into `NAMES`, so the numbers are the platform's (`SIGUSR2` is 12 on Linux; note
  `os.constants.signals` in `node/constants.ts` still hardcodes macOS numbers -- not touched).
  `kill(pid, sig)` is real `kill(2)`; when a process signals itself while a realm listens it
  waits for the arrival to be queued before returning (Linux runs the handler on the main
  thread, not the sender), which is what makes node's order deterministic. A realm's
  subscriptions end with its program (`release_thread`, called after the interpreter's graph
  run and after each `kiln_realm_run_*` realm thread).
- **TypeScript (`kiln-builtins/src/signals.ts`, one implementation, in the Node bootstrap
  before `web_events_streams.ts`).** node's `startListeningIfSignal` and `process.kill`. The
  engines' `process` emitters do not all emit `newListener` (the native one emits neither
  `newListener` nor `removeListener`), so `on`/`addListener`/`once`/`prepend*` on `process` are
  wrapped (own, non-enumerable) to run it first. Delivery: the unreferenced port wait wakes the
  realm, and the emit runs from `__kilnQueueIo` -- node's poll phase, after ticks and promise
  jobs, before immediates -- as `emit(name, name, number)`, one per arrival. Removing the last
  listener is noticed at the next arrival (no `removeListener` to hook): the realm gives the
  subscription up and the runtime restores the disposition and re-sends the signal, so it does
  what it would have done; only the timing differs (a realm in synchronous code dies of it when
  it reaches its loop). Workers (`node:worker_threads`) get `process.kill` but no signal events,
  as in node. `process.kill` replaces the two engines' stubs (interpreter: `true`, nothing sent;
  native: emitted the event synchronously).
- **Process mode.** The supervisor already forwarded; a worker with a listener now drains. When
  every worker exits by itself after a forwarded stop, the supervisor exits with their status
  (0 for a clean drain) instead of dying of the signal; any worker that died of a signal (no
  listener, or the grace SIGKILL) keeps the old answer (143 for SIGTERM).
- **Thread mode.** The `sigwait` thread hands each stop signal to `kiln_worker_op(20)`
  (`signals::deliver`); if any realm listens it arms a `KILN_WORKERS_GRACE_MS` timer (a detached
  thread) and keeps waiting, and the process ends when every realm has finished, or at the
  deadline by the signal's default action (`kiln: workers did not stop within N ms; ending the
  process`, 143 -- process mode's final status). No listener anywhere: dies of it as before.
  Other signals reach listening realms through the handler. Mixed (some realms listening, some
  not): the non-listeners keep serving until the listeners finish or the deadline -- a limit,
  since realms of one program normally all listen or all do not.
- **`process.exit` in thread mode** ends its realm, not the process: `workers::own_exits()` on
  each thread-mode realm thread makes `request_exit` record the code as for a worker, the realm
  unwinds, `kiln_cli_run_realm_as` returns the code, and the launcher applies the existing rule
  (0: others keep serving; non-zero: flush and `_exit(n)`). The drive now checks
  `exit_requested()` every turn and `kiln_worker_exit` wakes a parked drive, so an exit from a
  connection callback is immediate (it waited for the 200 ms introspection poll).
- **Two fixes the acceptance needed.** (1) Interpreter `server.close(cb)`: `close` was emitted
  when the listener closed, before in-flight responses, so `server.close(() => process.exit(0))`
  cut them off; now deferred to the last connection's `Closed` (node's rule). (2) Native: after
  an HTTP connection closed the drive was woken only when *no* host request was pending, so an
  unreferenced one (a signal listener's or an unref'd port's wait) left a virtual-clock realm
  parked forever; now woken unless referenced work remains.

**Verified** (Linux, release CLI from this branch):
- E2E, `server.mjs` (400 ms responses, 24 requests in flight, SIGTERM 150 ms in, handler
  `server.close(() => process.exit(0))`): node, interpreter, native single, `KILN_WORKERS=4`
  process and thread all `exit=0 ok=24/24`, port closed after (stop 250-290 ms native/node,
  433 ms interpreter). Variants: no listener -> 143 in all modes; listener that ignores it,
  grace 1000 ms -> 143 after 1003 ms (thread and process); listener removed after the first
  signal -> second SIGTERM 143 (single, thread, process); `process.exit(3)` in the handler -> 3
  (thread, process); SIGINT, no listener, thread mode -> 130.
- Regress (recorded from node 26.8.1, both engines byte-identical): `r37-signal-events` (event
  order vs ticks/jobs/immediates, `(name, number)`, `this`, `once`, SIGKILL/SIGSTOP errors,
  `process.kill` errors and signal 0), `r38-signal-drain` (in-flight request drained through a
  SIGTERM listener). `kiln test conformance/regress --node`: 38/38; `conformance/server` 6/6;
  `conformance/node` 36/36.
- Native corpus: `conformance/native/process-signal-events.cjs` (exit 0 through `process.exit`
  in a SIGTERM listener). `scripts/native.sh`: 270 matched, 1 refused (shadow-realm,
  pre-existing), 0 diverged.
- Crate tests: kiln-runtime 166 (+1, a real SIGURG round trip), kiln-interpreter 200,
  kiln-native 228, kiln-backend-c 190 + 36 (ABI-surface snapshot gains the four op defines),
  provider-ffi 37, kiln-builtins 98 + 4.

**Risks / left.**
- `kiln run` without `--one-shot` executes in the daemon, whose pid is `process.pid`: a program
  that now really `process.kill`s itself kills the daemon, and a listener subscribes the daemon
  (released when the program ends). The interpreter's CLI captures stdout until the program
  ends, so a program that dies of a signal prints nothing (pre-existing).
- `process.on` & co. are own properties of `process` now (the wrappers).
- Not run on macOS (signal numbers via `libc`, `__error`, `SIGINFO`/`SIGEMT` names) or as PID 1.
