# 07. `KILN_WORKERS` thread mode, and supervision for process mode

Status: ready-for-agent
Blocked by: 05 (thread mode); none (supervision)

**Thread mode.** `KILN_WORKERS=N` runs N realms of the program in one process instead of forking,
with accepts balanced across realms in process rather than by `SO_REUSEPORT` hashing. Once 08
lands, the realms share one copy of the built-ins. Process mode stays available
(`KILN_WORKERS_MODE=process`, name to settle) for crash isolation.

**Supervision for process mode**, which the launcher lacks today (`kiln-native/src/lib.rs`):
forward `SIGTERM` and `SIGINT` from the parent to every worker so graceful shutdown works in a
container; make workers exit when the parent dies; restart a worker that crashes; and give each
worker its index, so a program can run once-only startup work in one of them.

The `--workers` build flag decided in deploy issue 10 records `auto` or a count in the binary;
because the launcher's own comment warns that a baked count ties a binary to the build machine,
`kiln build` warns when a number rather than `auto` is recorded.

## Acceptance

- `docker stop` on a `KILN_WORKERS=auto` server drains every worker, in both modes.
- Killing one worker in process mode: it is restarted and the server keeps its capacity.
- Thread mode serves the small-service benchmark at least as fast as process mode, and reports its
  memory next to it.

## Comments

**2026-09-26 — supervision for process mode done (branch `alee/workers-supervision`); thread mode
remains.** All in the generated launcher `main` (`crates/native/kiln-native/src/graph.rs`, `kiln_sup_*`).

- **Shape.** With `KILN_WORKERS` > 1 the first process is a supervisor that runs *no* realm and
  forks N workers (indices 0..N-1). The parent could not keep its own realm: a restart must fork
  from a process that never built one, since a realm is provider threads plus process-global
  state (`kiln_namespaces`), and forking either hands the new worker locks another thread held
  and namespaces another realm filled. This is node cluster's primary, minus the program. N=1
  and Windows run the single realm exactly as before.
- **Signals.** SIGTERM/SIGINT/SIGHUP/SIGQUIT: an async-signal-safe handler records the signal and
  writes it to a self-pipe; the supervisor's `poll` loop forwards it to every live worker, stops
  restarting, waits for all of them, then dies of the same signal (`128+sig` when PID 1, where the
  default action never fires). Realms have no `process.on('SIGTERM')` today, so a worker does what
  a single-process server does, which is die of it; when realms grow a handler, the forward is
  already the right input. SIGUSR1 (introspection dump) is forwarded without stopping. Signals
  are blocked across `fork` and reset to default in the child before unblocking.
- **Parent death.** Linux: `prctl(PR_SET_PDEATHSIG, SIGTERM)` right after fork, then
  `getppid()` compared with the recorded supervisor to close the race. Elsewhere (macOS): a pipe
  whose write end only the supervisor holds, read by one detached thread per worker that sends
  itself SIGTERM on EOF. That branch compiles (checked with `-U__linux__`) but has **not been run
  on macOS**.
- **Restarts.** A worker that dies of a signal or exits non-zero outside shutdown is re-forked
  with the same index after 100 ms, doubling to a 2 s cap. The streak resets once a worker has
  lived 5 s; the sixth consecutive quick crash of one index stops every worker and exits with that
  worker's status (`128+sig` for a signal). Exit 0 is final and not restarted. Restarts are
  logged as `kiln: worker 0 (pid 3535505) was killed by signal 9 after 3157 ms; restarting in
  100 ms`. The supervisor also reaps orphans reparented to it as PID 1.
- **Index.** Each worker gets `KILN_WORKER_INDEX` and `KILN_WORKER_COUNT` in its environment
  before its realm starts (`process.env` in both engines, no new built-in). A single realm sets
  neither. Worker 0 writes the result file. Documented in README next to `KILN_WORKERS`.
- **`--workers`.** Untouched: the count still comes only from `kiln_cli_worker_count`, so a
  recorded `auto` or number becomes a default there and nothing in the supervisor changes.

Verified: `.scratch/realms-and-concurrency/probes/supervise-e2e.sh` against `serve-index.mjs`
(bench/server/serve.mjs answering with its index), built `--profile native-release`, Linux, all
PASS (`probes/e2e-result.txt`): four workers served with indices 0..3/4; `kill -9` of a worker
was restarted in 100 ms and all four indices served again; SIGTERM to the supervisor emptied
every worker and exited 143 in 2 ms; `kill -9` of the supervisor took every worker down and closed
the port. The same binary as PID 1 in a PID namespace (`unshare -Urpf`) exited on SIGTERM with
every worker gone. `native_build_launcher_supports_workers_auto` now also checks indices 0..2/3
under `KILN_WORKERS=3`, and `undefined` for both variables without it. `mbx test -p kiln-native
--offline`: 217 passed, 0 failed, 2 ignored (+4 passed).
