# PRD: Constant Conditional Branch Folding and CFG Unreachable Block Pruning

Status: completed
Feature: cfg-optimizations

## Problem Statement

When compiling JavaScript programs to native C via Kiln's SSA Value Graph (`FunctionGraph` in `kiln-backend-c`), conditional branching constructs (`if`, `? :`, `&&`, `||`, `??`) are initially emitted as `Terminator::Branch` or `Terminator::BranchNotNullish`.

Even after constant folding evaluates condition expressions to compile-time constants (e.g., `if (DEBUG)`, `if (true)`, `if (1 + 1 === 2)`, or constants derived from inlined functions / imports):
1. The conditional branches (`Terminator::Branch` and `Terminator::BranchNotNullish`) are currently never simplified into unconditional jumps (`Terminator::Jump`).
2. The untaken blocks and paths remain in `graph.blocks`, creating dead basic blocks, dead Phi nodes with spurious incoming edges, and dead local variables.
3. Downstream optimizations (dominator tree computation, loop preheader canonicalization, LICM, CSE, and type inference) waste CPU cycles analyzing dead blocks.
4. While the C emitter has defensive checks like `if (!block.is_reachable_from_entry(graph.entry_block)) { continue; }`, retaining unreachable blocks in the SSA graph bloats memory and prevents Phi simplification from converting single-source Phis into direct values.

## Goals & Technical Design

1. **Constant Condition Branch Folding (`fold_conditional_branches`)**:
   - Inspect `block.terminator`:
     - If `Terminator::Branch { condition, true_block, false_block }`:
       - If `true_block == false_block`: fold directly to `Terminator::Jump(true_block)`.
       - If `condition` is a known `Constant` (from `GraphOp::Const`):
         - If `constant_truthy(&constant)`: fold to `Terminator::Jump(true_block)` and eliminate edge to `false_block`.
         - Else: fold to `Terminator::Jump(false_block)` and eliminate edge to `true_block`.
     - If `Terminator::BranchNotNullish { condition, true_block, false_block }`:
       - If `true_block == false_block`: fold directly to `Terminator::Jump(true_block)`.
       - If `condition` is a known `Constant`:
         - If not nullish (`!matches!(constant, Constant::Null | Constant::Undefined)`): fold to `Terminator::Jump(true_block)` and eliminate edge to `false_block`.
         - Else: fold to `Terminator::Jump(false_block)` and eliminate edge to `true_block`.
   - Update `block.successors` to `vec![target]`.

2. **CFG Reachability & Unreachable Block Pruning (`prune_unreachable_blocks`)**:
   - Compute the full set of reachable blocks from `graph.entry_block` via BFS/DFS over `successors`, `exception_handler`, and `Terminator::Suspend` targets (`resume`, `delegate`), plus `graph.seam_block`.
   - For all reachable blocks:
     - Filter `block.predecessors` to retain only reachable blocks.
     - For `GraphOp::Phi { inputs }`: retain only inputs from reachable predecessors that remain in `block.predecessors`.
   - Retain only reachable blocks in `graph.blocks`.
   - Unlocked single-input Phis are subsequently simplified by `simplify_trivial_phis`, replacing Phi users directly with the single remaining source value.
   - Run `eliminate_dead_code` to eliminate operations that were only used in pruned blocks.

3. **Verification**:
   - Unit tests for:
     - Constant truthy branch folding (`true` condition -> Jump to `true_block`, `false_block` pruned).
     - Constant falsy branch folding (`false` condition -> Jump to `false_block`, `true_block` pruned).
     - Nullish branch folding (`BranchNotNullish` with null/undefined vs non-nullish constant).
     - Trivial phi reduction following dead branch elimination.
     - Preservation of exception handlers and suspend resume targets.
   - CFG Verifier check: `verify_function_graph` must pass on all pruned graphs with 0 errors.
   - Full workspace test pass (`kiln-backend-c`, `kiln-native`).
   - Native parity (`scripts/native.sh`) and thirdparty bundles (`scripts/thirdparty.sh`).
   - Quick gate (`mise run gate-quick`).
   - Zero changes to C runtime prelude (`crates/native/kiln-backend-c/src/runtime/`).
