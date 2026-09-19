# PRD: Phase 0 — binary size visibility

Status: closed (Phase 0 accepted 2026-09-12 on `work/sizes`; see `phase1-PRD.md` for the phase that followed)

## Problem

`kiln build` reports only aggregate bytes (generated C, objects, executable).
Developers cannot see *which modules* contribute size (e.g. why an Effect
hello-world is 19MB). No size tracking exists, so growth is invisible.

## Scope (Phase 0 only — no behavior change)

1. `kiln build --explain` appends a per-module retained-size table for the
   graph path: per module, live/dropped, total vs retained IR instructions
   and functions, sorted by retained size descending.
2. `scripts/binary-sizes.sh --check/--update` + `bench/results/binary-sizes-baseline.json`:
   builds a fixed fast target set, records executable bytes, diffs with tolerance.

Out of scope: any shaking/linking change, prelude work, kiln-facts changes,
ProgramFacts schema changes.

## Design

- `kiln-backend-c` facts.rs: new `ModuleSizeFacts` struct + `format_module_sizes()`.
  ProgramFacts untouched (no baseline churn).
- `kiln-native`: new `pub fn executable_module_sizes(graph, profile)` — thin
  wrapper over existing `native_executable_graph` + `retained_module_size`.
  Explain recomputes liveness out-of-band; never fails the build (skip section on error).
- `kiln-daemon` `native_build_graph`: append section when `explain`.
- Single-program path: unchanged (no graph, no demand).

## Acceptance — done 2026-09-12, branch `work/sizes`

- `kiln build --explain` on a multi-module program shows the table; dropped
  modules listed as removed. Verified on a fixture and on Effect
  (`59 live, 196 dropped, 61049/412492 instructions`).
- Narrow tests pass (`kiln-backend-c` 128, `kiln-daemon` 106, new
  `kiln-native` sizes test, cli explain test). `cargo fmt` clean.
- `scripts/binary-sizes.sh --check` passes on a fresh baseline;
  deterministic (+0.0% re-run) and the failure path verified.
