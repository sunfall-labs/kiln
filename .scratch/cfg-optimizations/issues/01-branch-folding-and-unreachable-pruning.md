# Issue 01: Constant Conditional Branch Folding and Unreachable Block Pruning

Status: completed
Feature: cfg-optimizations

## Objectives
1. In `crates/kiln-backend-c/src/graph.rs`:
   - Implement `fold_conditional_branches(graph: &mut FunctionGraph) -> bool`:
     - Constant truthy/falsy evaluation on `Terminator::Branch` and `Terminator::BranchNotNullish`.
     - Update `terminator` to `Terminator::Jump(target)` and `successors` to `vec![target]`.
     - Update untaken block's `predecessors` and `Phi` inputs.
   - Implement `prune_unreachable_blocks(graph: &mut FunctionGraph)`:
     - Reachability traversal from `entry_block` following successors, exception handlers, suspend targets, and seam block.
     - Prune unreachable blocks from `graph.blocks`.
     - Filter predecessors and Phi inputs in surviving blocks.
   - Integrate `fold_conditional_branches` into `build_function_graph` optimization pipeline.
2. In `crates/kiln-backend-c/src/graph.rs` tests:
   - Add unit tests for `fold_conditional_branches` (truthy, falsy, nullish, phi simplification, unreachable block pruning).
3. Verify:
   - `mbx test -p kiln-backend-c --lib`
   - `mbx test -p kiln-backend-c --test emitted_c`
   - `mbx test -p kiln-native`
   - `./scripts/native.sh`
   - `./scripts/thirdparty.sh`
   - `mise run facts`
   - `mise run gate-quick`
