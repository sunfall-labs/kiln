Status: closed
Feature: open-world-speculation

# 03 - Specialize `.length` Reads and Array Indexing

## Goals
1. **Specialize `.length` reads**:
   - In `properties.rs` `length_check`: add O(1) inline support for `kiln_typed_array` via `view->length`.
   - In `direct.rs` `intrinsic_results`: recognize `GetProperty` on definite arrays (`NewArray`) and strings (`Constant::String`, `TypeOf`) as `Proven::Int`.
   - In `graph.rs` `proven_types_with_bounds`: support `length_bounds` during type settlement so loop counters bounded by `.length` are proven as `InductionVariable` and `Proven::Int`.
2. **Open-World Array Indexing (`arr[i]` and `arr[i] = v`)**:
   - In `properties.rs` `emit_get_computed` and `emit_set_computed`: decouple `vk_decl` from `spec.arrays` so any integer key synthesizes `vk` on demand only if `!handled`, leaving `i{k}` unboxed.
   - In `body.rs` `boxed_uses`: do not force `key` to box for `GetComputed` or `SetComputed` when `ints.contains(key)`.
   - In `body.rs` `length_bounds`: include any `GetProperty` of `"length"` to enable induction variable detection across loops.
   - In `body.rs` `feeds_arithmetic`: include comparisons (`<`, `<=`, `>`, `>=`) and bitwise operations to allow versioned numeric speculation on properties that feed loop conditions.

## Acceptance Criteria
- Unit tests pass: `mbx test -p kiln-backend-c`.
- Snapshot tests pass.
- Parity suite passes: `./scripts/native.sh`.
- Quick gate passes: `mise run gate-quick`.
- Executable unit tests verifying unboxed `.length` and array indexing.
