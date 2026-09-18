# PRD: SSA Constant Folding and Generalised CSE in FunctionGraph

Status: completed
Feature: ssa-optimizations

## Problem Statement

The Kiln SSA Value Graph (`FunctionGraph` in `kiln-backend-c`) was introduced in ADR 0056 and refined in ADR 0061. It forms the core intermediate representation for native code generation, providing explicit SSA dataflow, verified dominance, effect tracking, and optimization passes (DCE, LICM, loop preheader canonicalization, and Dominator-Tree CSE).

However, three key optimization capabilities remain missing or artificially restricted:
1. **Zero Constant Folding**:
   Neither `kiln-ir` nor `FunctionGraph` performs constant folding on unary or binary operations. Expressions like `const M = 10 * 1024 * 1024;`, `const MASK = 1 << 8;`, `!0`, `void 0`, and negative numeric literals `const N = -1;` result in multiple runtime instructions, function calls (`kiln_c99_binary`), and redundant SSA values in emitted C.
2. **Artificial Block Bound on CSE**:
   `eliminate_common_subexpressions` currently starts with `if graph.blocks.len() < 2 { return; }`. This completely disables common subexpression elimination on all single-block functions (e.g. mathematical formulas, point transformations, getters, string formatters, and utility functions) even when identical subexpressions are repeated within the block.
3. **Identical Phi Deduplication**:
   When multiple local variables merge the same values across predecessor edges, identical `Phi` nodes are emitted in the target block. While `simplify_trivial_phis` simplifies single-value phis, it does not deduplicate identical multi-input phis in the same basic block.

## Goals & Technical Design

1. **SSA Constant Folding (`fold_constants`)**:
   Implement a forward propagation pass over `FunctionGraph` that evaluates pure operations on known `Constant` values:
   - **Unary Operators**:
     - `UnaryOp::Not`: boolean inversion (`Constant::Boolean(!truthy)`).
     - `UnaryOp::Void`: returns `Constant::Undefined`.
     - `UnaryOp::Negate`: numeric negation on `Constant::Number`.
     - `UnaryOp::Plus`: numeric coercion on `Constant::Number`, `Constant::Boolean`, `Constant::Null`.
     - `UnaryOp::BitwiseNot`: 32-bit bitwise NOT (`~n`).
   - **Binary Operators**:
     - Arithmetic (`Add`, `Subtract`, `Multiply`, `Divide`, `Remainder`, `Exponentiate`) on `Constant::Number` (IEEE-754 semantics matching ECMAScript).
     - String concatenation (`Add`) on `Constant::String`.
     - Bitwise operators (`BitwiseAnd`, `BitwiseOr`, `BitwiseXor`, `ShiftLeft`, `ShiftRight`, `UnsignedShiftRight`) with 32-bit ToInt32/ToUint32 semantics.
     - Strict comparisons (`StrictEqual`, `StrictNotEqual`, `LessThan`, `LessThanOrEqual`, `GreaterThan`, `GreaterThanOrEqual`) on numbers, booleans, null, and undefined (with proper IEEE-754 `NaN` rules).
   - Replaces folded operations with `GraphOp::Const`, allowing subsequent operations in the graph to fold transitively. Dead constant definitions are subsequently eliminated by `eliminate_dead_code`.

2. **Single-Block and Dominator CSE**:
   - Change `if graph.blocks.len() < 2` in `eliminate_common_subexpressions` to `if graph.blocks.is_empty()`.
   - The dominator tree correctly handles single-block graphs (`entry_block` dominates itself, child list is empty), allowing intra-block CSE across all operations in program order.
   - Extend `CseKey` with `CseKey::Phi(BlockId, Vec<(BlockId, ValueId)>)` to detect and deduplicate identical Phis within the same block.

3. **Verification**:
   - Unit tests covering all folded arithmetic, bitwise, comparison, and unary operations.
   - Unit test verifying CSE deduplication in single-block functions.
   - Unit test verifying identical Phi deduplication.
   - 100% pass on workspace tests (`kiln-backend-c`, `kiln-native`, `emitted_c`).
   - 100% pass on native parity (`scripts/native.sh`) and thirdparty bundles (`scripts/thirdparty.sh`).
   - Quick gate (`mise run gate-quick`).
