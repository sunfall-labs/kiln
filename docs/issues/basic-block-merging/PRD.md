# PRD: Sequential Basic Block Merging (Fallthrough Consolidation)

Status: completed
Feature: basic-block-merging

## Problem Statement

In the Kiln SSA Value Graph (`FunctionGraph` in `kiln-backend-c`), bytecode translation, branch lowering, loop preheader canonicalization, and conditional branch folding frequently leave sequences of basic blocks where Block A unconditionally jumps to Block B (`Terminator::Jump(B)`), and Block B has exactly one predecessor (Block A).

Examples of such patterns include:
1. Entry block jumping unconditionally to the first real code block.
2. Constant-folded conditional branches where the taken branch was a jump to a target block whose only incoming edge is now from the folded branch.
3. Fallthrough blocks after loop exits or straight-line statement sequencing.

In emitted C99 code, each such unmerged block introduces:
- A separate C label (`kiln_block_X:;`).
- An unnecessary jump (`goto kiln_block_X;`).
- Disjoint basic blocks that inhibit intra-block optimizations (such as Common Subexpression Elimination and instruction scheduling).
- Increased C code size and compilation overhead in Clang.

## Goals & Technical Design

1. **Sequential Block Merging (`merge_basic_blocks`)**:
   Identify pairs of basic blocks `(A, B)` in `FunctionGraph` satisfying:
   - `A.terminator == Terminator::Jump(B.id)`
   - `A.id != B.id`
   - `B.predecessors.len() == 1 && B.predecessors[0] == A.id`
   - `A.exception_handler == B.exception_handler` (preserving exception handling boundaries)
   - B is not a protected target:
     - `B.id != graph.entry_block`
     - `Some(B.id) != graph.seam_block`
     - B is not an exception handler of any block in the graph
     - B is not a `Terminator::Suspend` target (`resume`, `delegate`)
   - B is not a `Suspend` terminator block (`!matches!(B.terminator, Terminator::Suspend { .. })`)
   - B has no Phis (or single-input Phis are already simplified)

2. **Merge Transformation**:
   - Append `B.operations` to `A.operations`.
   - Update `A.terminator = B.terminator`.
   - Update `A.successors = B.successors`.
   - For every successor `succ` of B:
     - In `succ.predecessors`: replace `B.id` with `A.id` (deduplicating if `A.id` is already present).
     - In `succ.operations` (`GraphOp::Phi { inputs }`): replace `(B.id, val)` with `(A.id, val)`.
   - Remove B from `graph.blocks`.
   - Iterate to fixpoint.

3. **Invariants & Safety**:
   - SSA dominance is strictly maintained: all definitions in A precede appended definitions in B; uses in B of A's definitions remain valid intra-block uses.
   - CFG symmetry is preserved: `succ.predecessors` and `A.successors` mirror each other.
   - Full pass through `verify_function_graph`.
   - Zero changes to C runtime prelude (`crates/native/kiln-backend-c/src/runtime/`).

## Verification Criteria
1. Unit tests in `crates/native/kiln-backend-c/src/graph.rs` verifying sequential block merging, verifier pass, and edge cases.
2. `mbx test -p kiln-backend-c --lib` passes with 100% success.
3. `mbx test -p kiln-backend-c --test emitted_c` passes (review snapshots).
4. `mbx test -p kiln-native` passes.
5. `./scripts/native.sh` parity passes.
6. `./scripts/thirdparty.sh` passes.
7. `mise run facts` / `facts_baseline` passes.
8. `mise run gate-quick` passes cleanly.
