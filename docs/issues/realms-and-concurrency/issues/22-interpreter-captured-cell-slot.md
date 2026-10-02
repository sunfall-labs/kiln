# 22. Interpreter: "local slot 0 does not hold a captured cell"

Status: resolved

Found by issue 06: `bench/concurrency/js/inrealm.mjs` fails in the interpreter with "local slot 0
does not hold a captured cell" from `bench/concurrency/js/common.mjs`, with or without workers; the
native build runs it. Reduce to a minimal `conformance/regress` or `conformance/guard` probe, fix the
interpreter, and check both engines agree.

## Acceptance

- The probe passes in both engines; `bench/concurrency/js/inrealm.mjs` runs interpreted.

## Comments

**2026-09-27 -- branch `work/signal-events`: resolved.**

Minimal form: `import fs from "./dep.mjs"; function f() { return fs; } console.log("ok");` --
a function *nothing calls* that names an import. The module graph's liveness drops the import
(nothing live reads it), so the loader seeds no cell in its slot; the body still hoists `f` and
its `MakeFunction` captures the slot, and `Frame::cell` refused the non-cell before the module's
first line ran. Calling `f`, or merely naming it, kept the import live and hid the bug.
`common.mjs`'s `rssKiB` (the only reader of `fs`, and `inrealm.mjs` never calls it) was exactly
this shape. The native backend never failed: its module locals start `undefined` and a dropped
import is read as that.

Fix, interpreter only (`execute_module_in_runtime_mode_with_session`,
`crates/interp/kiln-interpreter/src/execution_api.rs`): after placing the imports, every slot the
module body reads as a cell (a capture by a function it makes, `LoadCell`, `StoreCell`) that no
`NewCell` creates and that is still empty gets a cell holding `undefined` -- the same walk as the
C backend's `slots_read_as_cells`. The linker was not changed: dropping a dead import is right,
and keeping one would load a module nothing needs.

- Probe: `conformance/regress/r36-dead-closure-captures-import.mjs` (dead declaration and dead
  arrow over `node:fs`/`node:path` imports, a live capture of `node:util` beside them), recorded
  from node 26.8.1. Fails on a binary without the fix, passes interpreted and native
  (`--node --native`).
- `bench/concurrency/js/inrealm.mjs 0 100` now runs interpreted with node's checksums on every
  case. With a non-zero budget it does not end: `perOp` spins on `performance.now()`, and the
  interpreter's clock is virtual (ADR 0044), so that is the harness, not this bug.
