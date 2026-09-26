# 10. The multicore default

Status: ready-for-agent

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
