# PRD: Open-World Speculation & SSA Unboxing Completeness

Status: closed (landed 2026-09-18: `05e05bd9`, `64974705`; issues 01-03 closed)

## Problem Statement
In open-world execution (modules with dynamic imports, `eval`, or external dependencies), Kiln cannot assume closed-world whole-program facts. However, standard JavaScript applications spend significant CPU cycles in:
1. Mathematical computations (`Math.*`) and bitwise operations (`|`, `&`, `^`, `<<`, `>>`, `>>>`) that are forced to box intermediate variables into 64-bit `Value` structs.
2. High-frequency type predicates like `Array.isArray(x)` and `Number.isFinite(x)` / `Number.isNaN(x)` / `Number.isInteger(x)` that route through full dynamic method dispatch frames.

## Architecture & Design
Leverage Kiln's sound speculation model:
- Check `kiln_c99_builtin_store_epoch == 0` for speculative built-in inlining (single-cycle branch prediction).
- In `crates/kiln-backend-c/src/emit_graph/body.rs`, refine `boxed_uses` so operands to pure operations and specialized built-ins are not marked as `used`, allowing them to live purely in hardware registers (`d{id}` and `i{id}`).
- In `crates/kiln-backend-c/src/emit_graph/ops/call_method.rs`, inline fast paths for `Array.isArray` and `Number.*` predicates.
- In `crates/kiln-backend-c/src/emit_graph/clones.rs`, include the full pure Math surface in restartable fast clones.

## Scope
- **Tier 1**:
  - Full `Math.*` surface across `body.rs` (unboxing) and `clones.rs` (restartable clones).
  - Bitwise operations over integer SSA operands in `body.rs` (`twin_only`).
- **Tier 2**:
  - `Array.isArray(x)` inline fast path.
  - `Number.isFinite`, `Number.isNaN`, `Number.isInteger`, `Number.isSafeInteger` inline fast paths.
- **Tier 3**:
  - Specialize `.length` reads: TypedArray inline support, `Proven::Int` for definite arrays/strings in `direct.rs` and `graph.rs`.
  - Open-world array indexing (`arr[i]`, `arr[i] = v`): unbox integer keys across `GetComputed` and `SetComputed`, expand `length_bounds` to all `.length` reads, and expand `feeds_arithmetic` to comparisons and bitwise ops.
