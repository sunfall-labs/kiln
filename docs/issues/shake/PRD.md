# PRD: Phase 2 — shared-graph shaking (one shaken graph feeds both engines)

Status: closed (landed 2026-09-13: `1ede4122`, merged via PR #3)

## Problem

Closed-world liveness lives only in the native backend
(`native_executable_graph` in `kiln-native`, `closed_world_*` in
`kiln-backend-c`). The interpreter evaluates the complete graph: every
module body runs even when nothing can observe it. Two selections, two
maintainers, one of them missing.

## Goal

A single `ModuleGraph → ModuleGraph` reduction both engines consume:
- the interpreter skips dead modules (time + memory, same observables);
- native keeps today's emission decisions byte-for-byte;
- `build_shared_graph` (currently unpruned) gets pruning free.

## Design (narrowest seam)

New on `ModuleGraph` (`crates/compile/kiln-module-graph/src/lib.rs`):

```rust
pub struct LiveDemand {
    pub live: BTreeSet<PathBuf>,
    pub slots: BTreeMap<PathBuf, BTreeSet<u32>>,
}

impl ModuleGraph {
    /// Pure subgraph transform: filters `modules` + `order` to `live`
    /// (dependency-first relative order preserved, entry kept), rebuilds
    /// `export_resolutions` via the existing `resolve_exports`, re-runs
    /// `validate_links` so link errors stay pre-execution, carries the
    /// `patched_modules` overlay filtered likewise.
    pub fn reduce_to_live(&self, demand: &LiveDemand) -> Result<Self, ModuleGraphError>;
}
```

- The selection computation (`native_executable_graph` + `ExecutableDemand`)
  moves to the shared location (module-graph or a tiny new crate on top of
  it + `kiln-ir` link facts); `kiln-native` re-exports/uses it. No shape
  changes to `ModuleNode`/`ModuleProgram`/`Program`.
- Interpreter: `execute`/`execute_node`/`execute_mode`/`import_plan` take the
  already-reduced graph; the `order` loop skips dead modules with no
  per-engine liveness code.
- Native: `build_graph` replaces its internal selection threads with the
  reduced graph + shared demand; `build_shared_graph` gains the same call.
- CJS laziness: reduction must include every CJS edge retained bodies can
  reach (reuse `inactive_commonjs_dependencies` + `sideEffects` logic);
  an un-demanded CJS target is simply absent, which is correct only then.
- Slot-level `Nop` (native) rides the shared `slots`; the interpreter
  ignores it.

## Out of scope

- Purity model / statement-level shaking (Phase 3).
- kiln-db memoized reduction query (follow-up; reduction stays a pure
  function first, memoization second).
- `single-program` (non-graph) paths: no graph, nothing to share.

## Acceptance (in order)

1. **No-op refactor gate:** with reduction wired but selecting everything
   (`live` = all modules), native emission is byte-identical
   (`scripts/native.sh` + differential corpus green) and interpreter
   behavior is unchanged (guard/effect/regress/node corpora green).
2. **Selection moved:** native's selection deleted in favor of the shared
   computation; `native.sh` byte-identical again (proves the move, not a
   rewrite).
3. **Interpreter consumes reduced graphs:** dead modules skipped; corpora
   green; measured skip on an Effect-scale program (modules evaluated
   before/after via `--explain` or step counts).
4. `build_shared_graph` pruned; `binary-sizes.sh` green (no unintended
   growth).
