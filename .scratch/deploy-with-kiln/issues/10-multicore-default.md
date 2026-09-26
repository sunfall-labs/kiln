# 10. The multicore default

Status: ready-for-human

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
