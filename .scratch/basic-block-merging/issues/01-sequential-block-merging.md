# Issue 01: Implement Sequential Basic Block Merging in FunctionGraph

Status: completed
Feature: basic-block-merging

## Objectives
1. Implement `merge_basic_blocks(graph: &mut FunctionGraph) -> bool` in `crates/kiln-backend-c/src/graph.rs`:
   - Identify candidate pairs `(A, B)` where A unconditionally jumps to B, B's sole predecessor is A, exception handlers match, and B is not a protected target (entry, seam, handler, resume/delegate).
   - Merge B's operations, terminator, and successors into A.
   - Update predecessors and Phis in B's successors.
   - Remove B from `graph.blocks`.
   - Iterate to fixpoint.
2. Integrate `merge_basic_blocks` into `build_function_graph` optimization pipeline (e.g. after branch folding and dead code elimination).
3. Add unit tests in `crates/kiln-backend-c/src/graph.rs` asserting on:
   - Basic sequential merging of blocks.
   - Multi-block chain merging into a single block.
   - Protection of exception handlers, resume targets, and entry blocks.
   - CFG verification passing (`verify_function_graph`).
4. Validate full test suite:
   - `mbx test -p kiln-backend-c --lib`
   - `mbx test -p kiln-backend-c --test emitted_c`
   - `mbx test -p kiln-native`
   - `./scripts/native.sh`
   - `./scripts/thirdparty.sh`
   - `mise run facts`
   - `mise run gate-quick`
