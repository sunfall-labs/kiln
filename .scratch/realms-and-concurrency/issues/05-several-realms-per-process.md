# 05. Several pinned realms in one process

Status: ready-for-agent
Blocked by: 03, 04

With realm state separate (03, 04), run several realms in one native process, each pinned to its
own thread: the 1:1 model, which ADR 0070 keeps as pinning. `WorkerPool` in `kiln-runtime` is the
starting point; make it the realm host rather than a parallel implementation.

Test with a harness that runs N copies of each `conformance/native` probe as separate realms
concurrently in one process and requires each to print exactly what it prints alone.

## Acceptance

- The harness passes for N = 1, 2 and the core count, and under `KILN_GC_STRESS`.
- Nothing in the process-global inventory from 01 remains unclassified.
