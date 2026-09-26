# 03. Implement ADR 0040: the executor split

Status: resolved

ADR 0040 is accepted and not implemented. `RealmRuntime` owns both the Tokio executor and the realm
state, so the native drive re-enters the executor about 8.5 times per request and carries an
`_in_context` twin for eleven methods. Split them as the ADR decides: one executor entry per program
run, the drive holding `&mut RealmState`, and the `_in_context` family deleted.

This is a single-realm throughput change first. It is also the first step of ADR 0070: once realm
state is its own value, a realm can be driven by any thread.

## Acceptance

- The `_in_context` methods and the `in_context` forks in the adapter are gone.
- Runtime entries per request, measured as in ADR 0040, fall to about one.
- `scripts/native.sh` 0 diverged; the small-service benchmark does not regress.

## Comments

**2026-09-26, branch `alee/executor-split`.** Implemented; ADR 0040 now records it under *What
step 4 did*.

What was already true on `main` when this started: f9ea9574 had deleted the `_in_context` twins
and made the native drive a single `block_on` around `drive_async`. What remained was the
question those twins answered, centralised in `Executor::run` (`Handle::try_current()` or a
`block_on`), and an adapter that reached the realm through `RefCell<RealmRuntime>` plus a raw
`*mut RealmRuntime` thread-local taken with `as_ptr()`.

What changed:

- `kiln_runtime::Executor` is public: Tokio runtime, `LocalSet`, paused-clock flag, metric stream.
  `block_on` is the one parking entry; `scope` makes the executor ambient without parking
  (`Runtime::enter` + `LocalSet::enter`, both nest). `Executor::run` is gone.
- `RealmState::new(host, capabilities)` builds state with no executor. Every spawning operation
  (`start_host_op`, `try_start_host_op`, `server_attach`, `client_attach`, the queued attaches,
  `http_close`) and every state-only query the drive uses (`ready_turn`, `completion_signal`,
  `wake_drive`, `requests_in_flight`, the HTTP agent's five operations, ...) lives on
  `RealmState`. `realm_state_operations_never_park_the_thread` still passes over the larger block.
- `RealmRuntime` is `Rc<Executor>` + `RealmState` (`from_parts`, `executor()`), with a same-named
  wrapper per spawning operation that enters `scope`. `DriveExecutor`, `drive_executor()` and
  `split()` are gone; `RealmHostProvider::drain_on_shutdown` takes `&Executor`. Interpreter,
  daemon, conformance and module-graph compile unchanged; `worker_pool` changed one line.
- The native adapter holds `Rc<Executor>` and `RefCell<RealmState>` (`NativeRuntime`, whose drop
  drains and shuts down exactly as `RealmRuntime`'s did). `kiln_realm_drive` enters the executor
  once; `kiln_realm_enter` is a `block_on` on the same executor. `ACTIVE_RUNTIME`,
  `ActiveRuntime`, `active_runtime()` and all `as_ptr()` escapes into the runtime are deleted;
  callbacks go through the active realm and `try_borrow_mut`, failing with `RUNTIME_ERROR` on a
  re-entrant borrow instead of aliasing. The C ABI is unchanged.

Verified, once each:

- `mbx test -p kiln-runtime --offline`: 154 passed, 0 failed.
- `scripts/native.sh` (release CLI from this branch): 222 matched, 1 refused (`shadow-realm`,
  pre-existing), 0 diverged.
- `mbx check --tests` over kiln-runtime, kiln-interpreter, kiln-native, kiln-daemon,
  kiln-module-graph, kiln-conformance, kiln-native-provider-ffi: clean. The provider also builds
  with `--no-default-features --features realm,regexp,bigint` (no `realm-http`). Clippy on the two
  touched crates: 24 warnings, down from 26 on the base, none new.

Measured:

- Executor entries: 1 `kiln_realm_enter` + 1 `kiln_realm_drive` for 2,000 requests to
  `bench/server/serve.mjs` (gdb breakpoint hit counts on a `native-dev` build) -- one entry per
  program run, not per request. By the code this was already so on the base.
- Hello server, c=16, 5 s, interleaved: 14,156 / 15,168 rps before, 14,983 / 14,702 after (a third
  round was disturbed by other load on both sides). Tokio `park_count` about 2 per request on both.
- Small service (`app.mjs` + `drive-mix.mjs`, c=16): server CPU per request 61.7 / 50.4 / 49.0 µs
  before, 62.2 / 52.5 after in comparable rounds; the machine was at load average 13-17, so this
  rules out a large regression and cannot resolve a small one.

Left over / not done:

- The drive does not hold a single `&mut RealmState` across its awaits: Hyper tasks run the
  language while it is parked and the language calls back into the state, so it borrows per step.
  The state has exactly one path to it (the cell), which is the property the ADR wanted.
- The paused clock is per executor. Several realms with different clocks on one executor (ADR
  0070's scheduler) will need that decided; today there is one realm per executor.
- A realm's HTTP connection tasks are spawned on one executor's `LocalSet`, so the state is not yet
  movable between executors while connections are open. That is ADR 0070's work.
- No throughput gain: the per-request entries this issue targeted were removed by the earlier
  steps, so this change is structural.

