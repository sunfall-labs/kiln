# Issue 05: HIR Escape Analysis & SSA Purity Loop Lowering

Status: resolved
Feature: multicore-concurrency
Governing ADR: ADR 0056, ADR 0057, ADR 0058

Resolution 2026-09-13:
- Region escape analysis `check_isolate_escapes` implemented in `crates/syntax/kiln-hir/src/lib.rs` and wired into `kiln-frontend-oxc`, emitting diagnostics when an object passed to `isolate()` escapes via local aliases or closure captures.
- SSA effect classification and native C work-stealing loop lowering implemented in `crates/native/kiln-backend-c` via `parallel_safe`, static C chunk worker lowering, and `kiln_parallel_map` in the native provider archive across a private Rayon pool (ADR 0058 Application 2).
- Verified via `mbx test -p kiln-hir` and `conformance/native/parallel-map.ts`.

## Context
Rather than executing parallel loops or region transfers interpretively through JS microtask state machines, Kiln's AOT compiler can verify non-aliasing and purity at compile time, lowering parallel operations directly into scalar C11 loops.

## Requirements
1. **Region Escape Analysis (`crates/syntax/kiln-hir`):**
   - Add an escape analysis pass in `kiln-hir` for arguments to `isolate(expr)`.
   * Prove that the root object and its child properties do not escape into local `let` bindings or outer closures.
   * Emit a compile-time diagnostic if an alias escapes.
2. **SSA Effect Classification (`crates/native/kiln-backend-c`, ADR 0056):**
   * Inspect closures passed to `parallelMap()` and pure `Effect.forEach()`.
   * Verify that the closure mutates no outer variables and calls only pure functions.
   * Classify verified operations as `Effect::Pure` in the SSA Value Graph.
3. **Native C Work-Stealing Loop Generation:**
   * Lower `Effect::Pure` parallel collection loops directly to native C work-stealing chunks over Tokio/Rayon.
   * Bypass JS generator and fiber state machine overhead for pure numeric/data transformations.

## Acceptance Criteria
- Statically proven pure loops compile into scalar C multithreaded loops without operand stack boxing.
- Attempting to pass an aliased object to `isolate()` fails with a clear diagnostic naming the alias.
- Emitted C compiles cleanly with Zig, Clang, and GCC across Linux and macOS.
