# PRD: Advanced C-Backend Optimizations (Type Completeness, Guard Pruning, Dense Key Tables, Chained Devirtualization)

Status: complete
Feature: c-backend-advanced-opt

## Problem Statement

While Kiln achieves SOTA execution performance across many workloads, profiling emitted C for complex libraries and benchmarks reveals three specific codegen inefficiencies:
1. **Incomplete SSA Type Propagation & Redundant Object Guards**:
   - `proven_types_with_bounds` in `graph.rs` handles `NewArray` as `Proven::Object`, but completely omits `NewObject` (falling back to `None`).
   - `intrinsic_results` in `direct.rs` recognizes `.length` on `NewArray` and string constants, but fails to recognize `.length` or numeric bounds on array methods (`map`, `filter`, `slice`, `split`) or string methods (`substring`, `join`), or method return types like `indexOf` and `push` (`Proven::Int`).
   - In `properties.rs`, `emit_get_property` and `emit_set_property` always emit `if (KILN_VAL_IS_OBJECT(v{o}))` tag tests even when `o` is statically proven to be an Object (e.g. from `NewObject`, `NewArray`, or dominator type inference).
2. **Key Table Emission Bloat**:
   - In `lib.rs` `render_key_table`, when `symbol == "ku"`, hundreds or thousands of keys are interned via unrolled individual lines: `if ((ku_keys[i] = kiln_c99_intern_static_key(&ku_string_i)) == NULL) return KILN_C99_OUT_OF_MEMORY;`. In large modules (e.g. TypeScript, zod), this inflates generated C by thousands of lines and slows Clang parsing/compilation.
3. **Chained Intrinsic Devirtualization**:
   - In `call_method.rs`, array and string methods (`map`, `filter`, `slice`, `join`) always emit runtime checks `(v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY` even when `o` is known to be the result of a preceding array-producing operation in a chained call (e.g. `arr.map(...).filter(...)`).

## Architecture & Implementation Plan

### Issue 01: SSA Proven Type Completeness & Redundant Object Guard Pruning
- In `crates/kiln-backend-c/src/graph.rs`: Add `GraphOp::NewObject { .. } => Some(Proven::Object)` to `proven_types_with_bounds`.
- In `crates/kiln-backend-c/src/direct.rs`: Extend `intrinsic_results` to recognize:
  - `CallMethod` of `map`, `filter`, `slice` on arrays and `split` on strings as producing Arrays (`Proven::Object`).
  - `CallMethod` of `substring`, `slice`, `join` on strings/arrays as producing Strings (`Proven::String`).
  - `CallMethod` of `indexOf`, `lastIndexOf`, `push` as producing Integers (`Proven::Int`).
  - `.length` reads on results of array/string producing operations as `Proven::Int`.
- In `crates/kiln-backend-c/src/emit_graph/ops/properties.rs`:
  - When `object` is proven to be `Proven::Object` (via `defs` or `proven`), elide the redundant `if (KILN_VAL_IS_OBJECT(v{o}))` guard or simplify the fast-path condition.

### Issue 02: Dense Static Key Table Compaction
- In `crates/kiln-backend-c/src/lib.rs` (`render_key_table`):
  - For `symbol == "ku"`, emit `static const kiln_string *const ku_static_keys[] = { &ku_string_0, ... };` and an intern loop:
    ```c
    for (size_t i = 0; i < sizeof(ku_static_keys)/sizeof(ku_static_keys[0]); ++i) {
        if ((ku_keys[i] = kiln_c99_intern_static_key(ku_static_keys[i])) == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
    ```
  - Reduces generated C source size significantly for modules with many keys.

### Issue 03: Chained Intrinsic Devirtualization
- In `call_method.rs`:
  - When `object` is statically proven to be an Array (e.g. from `NewArray` or prior `map`/`filter`/`slice`), streamline the receiver kind check so that chained pipelines execute the fast C path without redundant fallback branching.

## Acceptance Criteria
1. `mbx test -p kiln-backend-c --lib` passes with 100% success.
2. Snapshot tests pass (`mbx test -p kiln-backend-c --test emitted_c`).
3. `scripts/native.sh` parity passes with 206 matched, 0 diverged.
4. `scripts/thirdparty.sh` bundles pass with 22/22 matched.
5. Zero regressions in `mise run gate-quick`.
6. Phase 5.1 C prelude freeze in `crates/kiln-backend-c/src/runtime/` strictly preserved.
