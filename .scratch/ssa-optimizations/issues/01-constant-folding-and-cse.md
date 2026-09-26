# Issue 01: SSA Constant Folding and Generalised CSE

Status: completed
Feature: ssa-optimizations

## Objectives
1. In `crates/native/kiln-backend-c/src/graph.rs`:
   - Implement `fold_constants(graph: &mut FunctionGraph)` with `fold_unary` and `fold_binary`.
   - Generalize `eliminate_common_subexpressions` to run on single-block functions (`if graph.blocks.is_empty() { return; }`).
   - Add `CseKey::Phi(BlockId, Vec<(BlockId, ValueId)>)` to deduplicate identical Phis in the same basic block.
   - Insert `fold_constants(&mut graph)` into `build_function_graph` optimization pipeline.
2. In `crates/native/kiln-backend-c/src/graph.rs` tests:
   - Add unit tests for unary and binary constant folding.
   - Add unit test for single-block CSE.
   - Add unit test for identical Phi deduplication.
3. Verify:
   - `mbx test -p kiln-backend-c --lib`
   - `mbx test -p kiln-backend-c --test emitted_c`
   - `mbx test -p kiln-native`
   - `./scripts/native.sh`
   - `./scripts/thirdparty.sh`
   - `mise run facts`
   - `mise run gate-quick`
