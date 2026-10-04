# 32. Prepared parallel calls preserve the callee read before arguments

Status: resolved

JavaScript evaluates a method property before its arguments. A prepared parallel call must keep
that saved method even when an argument expression replaces the prototype member. The 4096-item
probe now replaces `Array.prototype.parallelMap` in a comma-expression argument, then supplies a
pure callback. It verifies the original method produces all 4096 mapped values, the replacement
is never invoked, and the argument side effect runs exactly once. A separate callback-side
mutation verifies an in-progress call also continues using its original method; the two counters
are independent so the cases cannot affect each other's result.

The same probe retains coverage for an inherited getter returning a custom method and one returning
the canonical method. The former getter is called once and its saved result is used; the latter
does not get re-read by the pool guard. The focused planner test
`parallel_sites_without_compiling_are_the_builds_own` records two sites: the pure callback is
pool-eligible, while the reassigned callback is refused with `CapturesReassigned`.

Validation on 2026-10-04:

- `KILN_BIN=target/release/kiln KILN_JOBS=1 MBX_GC_AUTO=false scripts/native.sh conformance/native/parallel-prepared-saved-callee.js`
  — 1 matched, 0 refused, 0 diverged.
- The same command with `KILN_NATIVE_GC_STRESS=1` — 1 matched, 0 refused, 0 diverged, 0 stress
  probes skipped.
- Direct native output includes `argumentMutationResult:[4096,3,4098]`,
  `argumentEffects:1`, `argumentReplacementCalls:0`, and `mutatedResult:[2,3,4]`.
- The CLI used for the source-only fixture validation has SHA-256
  `e27a77ff2fb28a7509a9f1da8c99c92b5a6addecf3ff62bc0199c5f29b441269`; it was already built
  from the planner/emitter change at `f5ac37ec0111b18f5f9d42b9161af0c29e093746`.
- The planner test passed with
  `MBX_GC_AUTO=false KILN_JOBS=1 RAYON_NUM_THREADS=1 mise exec -- mbx test -p kiln-native parallel_sites_without_compiling_are_the_builds_own -- --nocapture`.

Implementation commit: `f5ac37ec0111b18f5f9d42b9161af0c29e093746`. The regression is in
[`parallel-prepared-saved-callee.js`](../../../../conformance/native/parallel-prepared-saved-callee.js).
