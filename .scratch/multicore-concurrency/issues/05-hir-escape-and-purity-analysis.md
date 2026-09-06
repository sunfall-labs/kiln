# Issue 05: HIR Escape Analysis & SSA Purity Loop Lowering

Status: done
Feature: multicore-concurrency
Governing ADR: ADR 0056, ADR 0057

## Context
Rather than executing parallel loops or region transfers interpretively through JS microtask state machines, Kiln's AOT compiler can verify non-aliasing and purity at compile time, lowering parallel operations directly into scalar C11 loops.

## Requirements
1. **Region Escape Analysis (`crates/kiln-hir`):**
   - Add an escape analysis pass in `kiln-hir` for arguments to `isolate(expr)`.
   * Prove that the root object and its child properties do not escape into local `let` bindings or outer closures.
   * Emit a compile-time diagnostic if an alias escapes.
2. **SSA Effect Classification (`crates/kiln-backend-c`, ADR 0056):**
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
