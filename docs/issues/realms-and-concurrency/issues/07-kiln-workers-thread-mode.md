# 07. `KILN_WORKERS` thread mode, and supervision for process mode

Status: resolved (default stays process mode; re-measure after issue 08)
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

**2026-09-26 — supervision for process mode done (branch `work/workers-supervision`); thread mode
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

Verified: `docs/issues/realms-and-concurrency/probes/supervise-e2e.sh` against `serve-index.mjs`
(bench/server/serve.mjs answering with its index), built `--profile native-release`, Linux, all
PASS (`probes/e2e-result.txt`): four workers served with indices 0..3/4; `kill -9` of a worker
was restarted in 100 ms and all four indices served again; SIGTERM to the supervisor emptied
every worker and exited 143 in 2 ms; `kill -9` of the supervisor took every worker down and closed
the port. The same binary as PID 1 in a PID namespace (`unshare -Urpf`) exited on SIGTERM with
every worker gone. `native_build_launcher_supports_workers_auto` now also checks indices 0..2/3
under `KILN_WORKERS=3`, and `undefined` for both variables without it. `mbx test -p kiln-native
--offline`: 217 passed, 0 failed, 2 ignored (+4 passed).

**2026-09-27 — the four supervision gaps closed (branch `work/supervisor-finish`); thread mode
still remains.** Same place, `kiln_sup_*` in `crates/native/kiln-native/src/graph.rs`.

- **SIGKILL escalation.** Every stop (a forwarded signal, a non-zero exit, a crash loop, a fork
  that keeps failing) arms one deadline, `KILN_WORKERS_GRACE_MS` after it began (default 10000,
  capped at a day; 0 kills at once; an unparseable value is reported and the default used). A
  worker still alive then gets `SIGKILL` sent to its process group, which also takes whatever it
  spawned, and is logged: `kiln: worker 3 (pid 2877686) did not stop within 1000 ms; sending
  SIGKILL`. The supervisor then exits exactly as it would have (143 for SIGTERM).
- **The restart rule.** An exit status is the program's answer, and a restart cannot change it:
  exit 0 finishes that worker and leaves the rest serving (as before); any non-zero status --
  `process.exit(n)`, an uncaught error's 1, an unsettled top-level await's 13 -- stops every
  worker (SIGTERM, then the deadline above) and becomes the supervisor's own status, which is
  what the same program run alone would have exited with and what node cluster users wire
  `cluster.on('exit')` to surface. Only a death by a signal the supervisor did not send is a
  crash and restarted with backoff: a segfault, an abort, the OOM killer, an operator's `kill` of
  one worker to recycle it. The crash-loop limit (six within 5 s of starting) now applies only to
  those and exits `128+signal`. Once any stop has begun nothing restarts, so the signals the
  supervisor forwarded never count. Kiln folds every other failure to 1, so no exit status can be
  told apart as "not chosen by the program"; a runtime that dies on its own dies of a signal.
- **Ctrl-C: workers get their own process groups** (`setpgid` on both sides of the fork). The
  alternative, staying in the supervisor's group and not forwarding a SIGINT the workers already
  had, needs to know which copy it is holding: `tcgetpgrp` against our group plus `si_code`
  (`SI_KERNEL` for a tty signal on Linux; macOS spells it differently), and it still cannot tell a
  process manager's `kill -INT -<group>` from the terminal. With separate groups a worker is
  signalled on one path only, the supervisor's, for every signal and every sender, and the
  SIGKILL above can name a worker's group. The cost is job control: a worker is not in the
  terminal's foreground group, so reading the terminal stops it with SIGTTIN and Ctrl-Z stops only
  the supervisor -- neither is something N realms sharing one terminal could use. Parent-death
  handling is unchanged (PR_SET_PDEATHSIG does not care about groups).
- **Once-only startup work** is not solved: a restarted worker 0 still runs it again, and the
  README now says so. A restart generation in the environment would let a program tell; nobody
  has asked for it yet.
- **`--workers`** is deploy issue 10's comment: `kiln build --workers=<n|auto>` records the
  default `kiln_cli_worker_count` falls back to.

Verified: `probes/supervise-e2e.sh <plain> <built --workers=auto>` on Linux, native-release, all
23 checks PASS -- (a)-(e) as before; (f) every worker leads its own group and SIGINT to the
supervisor's whole group (a terminal's Ctrl-C) exits 130 with nothing restarted; (g) with
`KILN_WORKERS_GRACE_MS=1000` and one worker SIGSTOPped (a stopped process never acts on SIGTERM;
native realms have no `process.on('SIGTERM')` to ignore it with), SIGTERM exits 143 after 1003 ms
with the escalation logged; (h) worker 2 exiting 1 stops the group by itself, status 1, no
restart; (i) the `--workers=auto` binary runs 16 workers on 16 cores with `KILN_WORKERS` unset and
2 with `KILN_WORKERS=2`. `mbx test -p kiln-native --offline`: 220 passed, 1 failed, 2 ignored on
the one run -- the failure was the new test asserting that workers 0 and 2 printed before worker
1's exit stopped them, which the rule itself makes a race; it now reads only worker 1 and passes
(rerun alone). `-p kiln-cli -p kiln-proto -p kiln-daemon`: all passed.

**2026-09-27 -- thread mode done (branch `work/workers-thread-mode`); process mode stays the
default, by measurement. Status `needs-triage`: acceptance 1 and 2 hold, 3 holds at 32+
connections but not at 16 (below), and the default wants the owner's sign-off.**

- **Shape.** `KILN_WORKERS=N` with `KILN_WORKERS_MODE=thread` runs N realms of the program in one
  process: the launcher (`kiln_threads_*` in `crates/native/kiln-native/src/graph.rs`) calls the new
  provider export `kiln_realm_run_workers` (ABI 47 -> 48, 139 exports), which is
  `kiln_realm_run_pinned` whose realm threads first call `kiln_runtime::join_listen_group()`.
  `KILN_WORKERS_MODE=process` (or unset) is the supervisor as before; any other value is reported
  and the default used.
- **Accepts** (`crates/host/kiln-runtime/src/listen_group.rs`). On a joined thread, `TcpListen`
  makes the realm's listener a member of the process's one socket for `(host, port)`: the first
  member binds it and starts a `kiln-accept-<port>` thread (poll on the socket and a wake pipe,
  `accept4` until `EAGAIN`); every accepted connection goes to the member with the fewest
  *active* connections, ties round-robin from the member after the last chosen. The connection's
  `ListenTicket` lives in the realm's `TokioHost` beside its resource and gives the count back on
  close, on the HTTP engine's release, or with the realm. With the ring the connection arrives as a
  `TcpRaw` exactly like a ring-accepted one, so `take_transport` puts it on the realm's own ring.
  A member that closes hands its queued connections to the others; the last one closes the socket.
  A realm listening twice on one address gets its own socket the second time; port 0 is one shared
  key, as in node's cluster. Threads that did not join (worker_threads workers, `KILN_REALMS`) bind
  as before. Least-active over plain round-robin: identical on the benchmark (keep-alive, every
  realm ends up with c/N connections either way), and it keeps a realm holding long-lived
  connections from being handed more; the cost is one relaxed load per member per accept.
  Why not SO_REUSEPORT: with 16 connections over 16 process-mode workers the kernel's hash gave
  `1 1 1 1 1 1 2 2 3 3` -- six workers idle (`ss -tnp` during a run); thread mode gives 1 each.
- **Env.** Each realm's own `process.env` gets `KILN_WORKER_INDEX`/`KILN_WORKER_COUNT`, put by the
  launcher right after `kiln_install_native_modules`, before any module runs
  (`kiln_cli_put_worker_env`, keyed by a `_Thread_local` index). The process `environ` is not
  touched, so this is node's worker semantics (a copy of the parent's env per thread); a
  worker_threads worker started from a realm gets its parent realm's `process.env` copy through the
  existing INIT path. Worker 0 writes the result file.
- **Signals.** SIGTERM/INT/HUP/QUIT are blocked before the realm threads exist and taken by one
  `sigwait` thread, which ends the process by the same signal (SIG_DFL + raise) -- every realm at
  once, the equivalent of forwarding to N workers that each die of it -- and `_exit(128+sig)` as
  PID 1. SIGUSR1 untouched. There is no grace deadline: nothing survives the signal to wait for.
- **Exits.** Realm finishing 0: the others keep serving. Non-zero (uncaught error's 1, 13): `kiln:
  worker <i> exited with status <n>; stopping the other workers`, then `_exit(n)`. `process.exit(n)`
  in a realm is the prelude's `exit(n)`, which ends the process with n (tested). **A crash in any
  realm takes the process down**; nothing restarts -- that is process mode's job.
- **Output.** `console.log` from several realms interleaved mid-line on the shared stdout
  (`orkers-ok 1/3` / `wworkers-ok 2/3` in the first test run): the CLI host wrote a line as several
  `fputc`/`fwrite` calls. `kiln_cli_host_call` now holds `flockfile` from the first byte to the
  flush (`kiln-native-runtime`).

**Measured** (final tree, release `kiln`, `bench/server/serve.mjs` built `--profile
native-release`, `KILN_WORKERS=auto` = 16 on 8 cores/16 threads, `oha` 1.16 on the same machine,
load average 10-18 from other agents; each row three rounds, modes interleaved;
PSS = sum of `smaps_rollup` over the process tree).

| | thread | process |
|---|---|---|
| rps, c=16 | 164k / 163k / 164k | 179k / 178k / 188k |
| rps, c=32 | 228k / 226k / 230k | 232k / 228k / 233k |
| rps, c=64 | 287k-313k (6 runs) | 281k-308k (6 runs) |
| rps, c=256 | 262k / 369k / 369k | 354k / 374k / 374k |
| p99, c=16 | 0.13 ms | 0.12-0.13 ms |
| p99, c=64 | 1.23-1.57 ms | 1.05-1.19 ms |
| p99, c=256 | 3.8-8.0 ms | 3.5-4.0 ms |
| PSS idle | 39.1-40.0 MB | 49.8-51.0 MB |
| PSS after load | 180.1-182.0 MB | 190.9-194.9 MB |
| RSS sum after load | 183-185 MB | 405-419 MB (double counts shared text) |
| SIGTERM | 143, 2-21 ms, port closed | 143, 0-4 ms, port closed |

Where thread mode's c=16 deficit comes from: server CPU per request (from `/proc/<pid>/stat`,
c=16) is 34.6 us in thread mode against 27.6 us in process mode, but `KILN_REALMS=16` -- the same
sixteen realm threads in one process, each binding its own SO_REUSEPORT socket -- measures 27.8-30.2
us and 179-184k rps, i.e. the in-process hosting is not the cost. The balancing is: hashing packs
16 connections onto 10 workers, whose loops then serve 2-3 connections per wakeup and leave six
cores to the client, while even spreading wakes 16 realms once per request and contends with
`oha` for every core. Pinned apart (server `taskset 0-7`, `KILN_WORKERS=8`; `oha` on 8-15), c=32:
thread 242k/273k, process 270k/271k, CPU per request equal (16.6-17.7 us). TLB shootdowns during a
5 s run are equal (2614 vs 2578), so the shared address space is not it either.

**Default: process.** Throughput is a tie at 32 connections and up, thread mode is 8% behind at
16 and has the higher p99 at 64, and it saves 11-14 MB of PSS over 16 workers (0.7 MB a worker),
which does not pay for losing restarts and crash isolation. The memory argument for thread mode is
issue 08 (realms sharing the built-ins); the default is one `#define`
(`KILN_CLI_DEFAULT_WORKERS_THREADS`) to flip when it lands. The deploy decision (one realm unless
`--workers`) is unchanged; this only picks what N workers are.

**Verified.** Crate tests: kiln-runtime 165 passed (+3: `listen_group` least-active dispatch and
freeing, a closing member's queue handed on and the last closing the port, a second listen in one
realm binding its own socket); kiln-native 228 passed, 0 failed, 2 ignored
(`native_build_launcher_supports_workers_auto` now checks indices 0..2/3 in thread, process and
default mode; the recorded-count test also checks thread mode's `process.exit(7)`); kiln-backend-c
189 + 36; provider-ffi 37; provider-contract 11 + 5; kiln-native-runtime 16.
`scripts/native.sh`: 268 matched, 1 refused (shadow-realm, pre-existing), 0 diverged. E2E on the
built server: default mode forks 4 workers and SIGTERM exits 143; thread mode is one process, 19
threads, one listening socket; SIGTERM under 64-connection load exits 143 in 14 ms with the port
closed; SIGINT exits 130; as PID 1 (`unshare -Urpf --mount-proc`) it serves and SIGTERM exits 143
in 1 ms.

**Risks / left.**
- `process.exit(n)` in a thread-mode realm is C `exit` while other realm threads run (atexit
  handlers and stdio teardown under them); it worked in every run here, but `_exit` after a flush
  would be the robust spelling -- that is in the prelude (`dispatch_part_a.c`), left alone.
- No graceful drain in either mode: realms have no `process.on('SIGTERM')`, so in-flight requests
  die with the process, as they die with each worker in process mode.
- The accept thread is one thread per shared address; connection-per-request loads go through it
  (keep-alive loads touch it once per connection). Not measured with `Connection: close`.
- Provider ABI 48 / 139 exports will collide with any concurrent branch that appends exports.
- Not run on macOS (the `accept` + `fcntl` branch and `flockfile` declaration compile paths).

### 2026-09-27, integrator: decision

Thread mode lands opt-in (`KILN_WORKERS_MODE=thread`); process mode stays the default on the
measurements above: throughput ties from 32 connections, thread mode trails by 8% at 16 and has a
worse p99 at 64, and saves only ~0.7 MB per worker today, which does not pay for losing restarts and
crash isolation. The memory case for thread mode is issue 08 (realms sharing the built-ins): re-run
this measurement when 08 lands and flip `KILN_CLI_DEFAULT_WORKERS_THREADS` if thread mode then wins
on memory without losing throughput. Graceful drain on SIGTERM needs `process.on('SIGTERM')` in
realms, which is issue 23.
