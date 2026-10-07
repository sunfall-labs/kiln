# Native remainder by zero differs between map and parallelMap

Status: ready-for-agent
Resolution: fixed in the follow-up optimization pass; retained as regression evidence.

Discovered by the existing randomized `parallel_map_is_map_bit_for_bit` test during
build-loop validation on 2026-09-26. A deterministic three-element reproduction also
fails on unchanged main `d2c0e686ca904d5070ea83d83f2cbe35e461bf7a`; this predates the
build-loop and batch-scheduler changes.

Expression: `(818662.575431119 ^ element) % (1e-310 >> index)` over
`[4294967295, 0, 30]`. Native `parallelMap` returns three `NaN`s; native `map`
returns three zeros. The right operand is zero; JavaScript remainder should be `NaN`.

[Minimal test patch](../remainder-reproducer.patch) appends a deterministic use of the
existing comparison fixture. Apply in an owned worktree and run:

```sh
mbx test -p kiln-backend-c slow_gate_remainder_probe -- --nocapture
```

The temporary test was removed from the baseline worktree after reproduction. No
production fix, skipped test, changed baseline or reduced fuzz budget is included in
the build-loop work. The property test remains enabled in the routine gate.

Acceptance: retain a deterministic regression, identify and correct the differing
numeric lowering, and verify both paths match JavaScript including zero divisors,
NaN, infinities and signed zero. Keep the full gate as the final check.

Evidence:
- [Unchanged-main failure](~/Developer/kiln-fast-gate/target/unchanged-remainder-probe.log)
- [Original randomized failure](~/Developer/kiln-fast-gate/target/gate-logs/run.0yLYlGBs/01-crate_tests.log.raw)

## Comments

The follow-up optimization pass fixed integer-specialized remainder boxing. When no
double consumer existed, the emitter boxed the integer twin, which represents neither
NaN nor negative zero. Boxed remainder results now preserve both explicitly. The original
property test remains enabled, with a deterministic regression alongside it and a native
corpus probe that inspects the values after storage in a mapped array.

The deterministic test fails before the fix and passes after it; the native probe matches
the interpreter, and the interpreter output matches Node. Evidence is retained under
`~/Developer/kiln-slow-case-opt/docs/issues/slow-case-opt/remainder-{before,after,native}.log`.
