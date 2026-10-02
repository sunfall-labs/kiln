# 26. A program run inside the daemon must not signal the daemon

Status: resolved

Found by issue 23: `kiln run` without `--one-shot` runs the program inside the compiler daemon, so
`process.pid` is the daemon's pid, and `process.kill(process.pid, "SIGTERM")` with no listener now
really kills the daemon (before issue 23, `process.kill` did nothing). A program should observe
exactly what it would observe run alone: signals it sends to itself reach its own realm (its
listeners, or its own termination as the program's exit status), never the daemon, and
`process.pid` should not leak the daemon's identity where it matters. Decide the rule (for example:
a daemon-hosted realm gets a virtual pid, and self-directed `kill` is delivered in-realm; signals to
other pids stay real) and implement it.

Also from issue 23: `os.constants.signals` is hard-coded to macOS numbers; use the platform's.

## Acceptance

- `kiln run` (daemon) and `kiln run --one-shot` produce the same stdout, stderr summary and exit code
  for a program that signals itself with and without listeners; the daemon survives.
- `os.constants.signals` matches node on Linux and macOS.

## Comments

**2026-09-27 -- branch `work/daemon-signals`: resolved.** Protocol 35 -> 36 (`RunSummary.signal`).
No provider ABI, method id or `__kilnWorkerOp` op change; no native code touched.

**The rule** (`kiln_runtime::signals`, module docs "A program that is not the process"; README
"Using it"). Every interpreted run the daemon dispatches (`execute_prepared_run`, which one-shot
reaches too) and every conformance case runs under `signals::host_program()`, a per-thread
program that `node:worker_threads` workers inherit.

- *Resident daemon* (`kiln daemon serve` calls `Daemon::isolate_hosted_programs()`): the program
  is a virtual process. `process.pid` (and the `(node:<pid>)` warning prefix) is a virtual pid,
  one per run, from 2^22 up -- above Linux's `PID_MAX_LIMIT` and macOS's 99,999, so it never
  names a real process and `kill` of it from outside answers `ESRCH`. Logging it, comparing it
  with itself and signalling it behave as alone; `/proc/<pid>`, `ps` and a child's
  `process.ppid` see nothing or the daemon. A signal it sends to itself -- its pid, the
  daemon's real pid, `0`, `-1`, or `-<daemon's pgrp>` -- is delivered in the realm and never with
  `kill(2)`: to its listeners (queued on its endpoints before `kill` returns, so issue 23's
  event order holds), or else the signal's default action happens to the program: terminating
  -> the engine unwinds as `process.exit(128+n)` right at the `process.kill` call, the daemon
  reports `exit_code = 128+n, signal = n`, and the client writes the program's stdout/stderr and
  then re-raises `n` on itself (`WIFSIGNALED`, 143 in a shell -- node's signal death);
  ignoring (`SIGCHLD`, `SIGURG`, `SIGWINCH`, `SIGCONT`, macOS `SIGINFO`) -> nothing; stopping
  (`SIGSTOP`, `SIGTSTP`, `SIGTTIN`, `SIGTTOU`) -> nothing, the daemon cannot stop for one program.
  Its listeners install no process disposition, so a real `SIGTERM` to the daemon still stops
  the daemon. The last listener removed (issue 23's `unhandled`) applies the default action to
  the program instead of re-sending the signal to the process. Signals to any other pid: real
  `kill(2)`.
- *`--one-shot`* (the process is the program): real pid, real dispositions (an external
  `SIGTERM` still drains a server), but its self-directed signals (by its real pid) take the same
  in-realm path, and so does `unhandled` -- which is what lets `--one-shot` print what the program
  wrote before a signal ended it (before, the captured stdout died with the process).
- `os.constants` (new: `{ signals }`, frozen) and `node:constants`' `SIG*` come from one
  platform table in `node/constants.ts`, node's key order, only the platform's names (Linux adds
  `SIGSTKFLT`/`SIGPOLL`/`SIGPWR`, macOS `SIGINFO`).

**Verified** (Linux, debug CLI from this branch, my own daemon on a private `--socket`). Five
programs, each against node 26.8.1's stdout, stderr and exit code, under `kiln run --node`
(daemon) and `kiln run --one-shot --node`; all 10 identical to node:
`term-nolistener` (writes, then `process.kill(process.pid,'SIGTERM')`) 143 by signal, output
kept; `term-listener` (event order vs ticks/immediates/timer) 0; `usr2-listener` (`(name, 12)`,
`=== os.constants.signals.SIGUSR2`, `process.exit(7)`) 7; `removed` (listener removes itself and
re-sends) 143; `consts` (`os.constants.signals` JSON, `constants.SIGUSR2/SIGUSR1/SIGCHLD`,
`'SIGINFO' in constants`, frozen) 0 -- also identical through `--native`. The daemon's pid was
the same before and after the daemon runs. `kill(pid, 0)`, `SIGCHLD`/`SIGWINCH` to self: `true`,
no effect; a virtual pid is 4194309, 4194310, ... per run. `kiln test conformance/regress --node`:
38/38 one-shot and in the daemon.
Crate tests: kiln-runtime 167 (+1: a hosted program's virtual pid, delivery by its own, the
daemon's and pid 0, untouched disposition, default action on `unhandled`), kiln-interpreter 200,
kiln-proto 15, kiln-builtins 98, kiln-conformance 74 + 9, kiln-cli 162 + 12, kiln-daemon 108/109:
`testing::tests::changed_node_http_runtime_selects_only_the_http_builtin_cases` fails because
issue 23's `regress/r38-signal-drain` (it imports `node:http`) is now selected by a
`node_http.rs` change and the test's expected set predates it -- corpus, not this change.

**Limits / risks.**
- A worker thread of a hosted program that sends the program a terminating signal nobody listens
  for ends only the worker (as `process.exit(128+n)` would), not the main realm; node would end
  the process. The daemon is safe either way.
- `kill(0|-1|-pgrp, sig)` from a daemon-hosted program reaches only the program, not the rest of
  the client's process group; from `--one-shot` it is a real group kill, so a signal that ends
  the CLI that way still loses the captured stdout (pre-existing).
- `SIGUSR1` with no listener terminates (node starts its inspector instead); pre-existing.
- Signals sent to the `kiln run` client itself (Ctrl-C, `kill <client pid>`) are not forwarded to
  a daemon-hosted program; a request for that would be a protocol addition.
- Protocol 36: an older daemon refuses a newer client (and vice versa) -- the client restarts a
  stale daemon by build identity as before.
- Not run on macOS (virtual pid bound, `SIGINFO` ignore, macOS table taken from node's known
  values and the previous hard-coded ones).

