# Issue 01: SSA Proven Type Completeness & Redundant Object Guard Pruning

Status: closed
Feature: c-backend-advanced-opt

## Objectives
1. In `crates/native/kiln-backend-c/src/graph.rs` (`proven_types_with_bounds`):
   - Recognize `GraphOp::NewObject { .. } => Some(Proven::Object)`.
2. In `crates/native/kiln-backend-c/src/direct.rs` (`intrinsic_results`):
   - Recognize `indexOf`, `lastIndexOf`, and `push` on arrays as `Proven::Int`.
   - Recognize `substring`, `slice` (on string), and `join` as `Proven::String`.
   - Recognize `map`, `filter`, `slice` (on array), `split` as `Proven::Object`.
   - Recognize `.length` reads on results of array- or string-producing operations as `Proven::Int`.
3. In `crates/native/kiln-backend-c/src/emit_graph/ops/properties.rs`:
   - When `object` is statically proven to be an Object (via `defs.get(object)` being `NewObject` or `NewArray`, or `proven.get(object) == Some(&Proven::Object)`), emit the object IC path without wrapping in redundant `KILN_VAL_IS_OBJECT(v{o})` condition.
