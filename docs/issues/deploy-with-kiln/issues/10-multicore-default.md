# 10. The multicore default

Status: resolved

A kiln realm is one thread. Node and Bun are single-threaded for JavaScript too, but on the
README's Linux run Bun used twelve threads and Node five, and one kiln realm trailed Bun by 4% on
the small service. `KILN_WORKERS=auto` forks one realm per core over a shared `SO_REUSEPORT`
listener and wins, but it is opt-in.

Decide:

1. **The default for a built server.** Options: one realm (matches Node's model, and the user
   scales with processes or containers); `auto` (wins benchmarks, but changes the semantics of
   in-memory state and surprises anyone who expects one process); or `auto` only behind a
   `kiln build --workers` flag recorded in the binary.
2. **How numbers are reported.** Per core *and* whole machine, always side by side, so no table
   compares one kiln realm with twelve Bun threads or the reverse without saying so.

## Acceptance

- The decision recorded (an ADR if it changes the default).
- Issue 11's tables follow the reporting rule.

## Decision, 2026-09-26

Decided by the project owner:

1. **A built server runs one realm by default**, matching Node's model.
   `kiln build --workers=<n|auto>` records a worker count in the binary; `KILN_WORKERS` still
   overrides it at run time.
2. **Every published number shows per core and whole machine side by side**, with the thread or
   process count of each runtime stated.

The default does not change, so no ADR is needed. What remains is implementation: the
`--workers` build flag, its record in the binary, and the run-time override order.

## Acceptance (implementation)

- `kiln build --workers=auto app.ts` produces a binary that forks one realm per core with no
  environment variable set; a binary built without the flag runs one realm.
- `KILN_WORKERS` overrides the recorded value in both cases.

## Comments

**2026-09-27 — the flag, its record, and the override order (branch `work/supervisor-finish`).**

- `kiln build --workers=<n|auto>` (also `--workers <v>`; `auto` any case; 1..=1024; refused with
  `--shared`, and by a build that falls back to the single-program launcher, which has no
  supervisor) travels as `Request::NativeBuild.workers` (`RecordedWorkers`, off the wire when
  absent; `PROTOCOL_VERSION` 34 -> 35 because an older daemon would drop it silently) into
  `NativeRealmPolicy::workers`.
- The graph launcher records it as `#define KILN_CLI_RECORDED_WORKERS "auto"` (or the number),
  emitted only when set; `kiln_cli_worker_count` uses it when `KILN_WORKERS` is unset or empty.
  The launcher text is the launcher's cache key, so the record is part of the binary's identity
  exactly when it is set, and a build without the flag emits the same launcher text as before.
  `auto` is still resolved by
  `sysconf` on the machine that runs the binary.
- A number warns at build time: `kiln: warning: --workers=3 records a fixed count: this binary
  forks 3 workers on every machine it runs on, however many cores that is. `--workers=auto` sizes
  to the machine that runs it; `KILN_WORKERS` overrides either at run time.`
- README's `KILN_WORKERS` section documents the order.

Acceptance: a `--workers=auto` build of `serve-index.mjs` forked 16 workers on this 16-core box
with no variable set, and 2 under `KILN_WORKERS=2` (supervise-e2e.sh (i)); a build without the
flag runs one realm and `KILN_WORKERS=3` still gives three (`native_build_launcher_supports_workers_auto`);
a recorded `Count(3)` runs three with the variable unset and one under `KILN_WORKERS=1`
(`native_build_records_a_worker_count_and_surfaces_a_worker_exit_status`).
