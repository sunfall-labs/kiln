# 13. Parallel code in every module and every build profile

Status: ready-for-agent
Priority: first, before tiers 2-4

Found by issue 03 (`--explain-parallel`, reason `generic-only`): `kiln build` defaults to the dev
profile, which never emits typed or parallel code, and a release-profile module with more than 16
functions or 512 instructions is also emitted generic-only. So most real modules get no parallel
`parallelMap` whatever their callbacks, and every later tier multiplies nothing.

Find why the typed/direct emission is gated on module size and profile (compile time, code size,
the direct plan's cost), and make the parallel lowering of a qualifying `parallelMap` call site
independent of it: the callback and the call site need typed entries, not the whole module. Decide
whether the dev profile should emit parallel code at all (it is the edit-run loop; the interpreter
is the reference there), and record the decision.

## Acceptance

- A `parallelMap` call site with a qualifying callback runs in parallel in a large release module
  (a probe with more than 16 functions and 512 instructions).
- `--explain-parallel` no longer reports `generic-only` for release builds of the concurrency
  benchmark and the deploy corpus apps.
- Release build time of the thirdparty bundles within 10% of before.
