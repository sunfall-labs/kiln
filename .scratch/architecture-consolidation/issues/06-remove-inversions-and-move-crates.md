# 06. Remove the layer inversions and move crates into layer directories

Status: ready-for-agent
Blocked by: 05

In this order, each its own change:

1. **Module evaluation leaves `kiln-module-graph`.** `ModuleGraph::execute`, `execute_node` and
   their use of about thirty `kiln_interpreter` items move to layer 4a. `kiln-module-graph` loses
   its `kiln-interpreter` and `kiln-runtime` dependencies. Check: `kiln-backend-c` no longer
   builds `kiln-interpreter` (`cargo tree -p kiln-backend-c -e normal | grep interpreter` is empty).
2. **`kiln-liveness` stops depending on `kiln-backend-c`.** Backend-neutral facts
   (`DemandedMembers`, method groups, the `closed_world_*` queries) move to layer 3; C-specific
   sizes arrive through a trait the native layer implements.
3. **`kiln-db` stops depending on `kiln-backend-c`.** `summarize_program` and `summaries_digest`
   register as queries from the native layer.
4. **Number-provider consolidation** as ADR 0066 describes.
5. **The directory move.** One change, a pure `git mv` into `crates/<layer>/`, plus
   `Cargo.toml` workspace members and every script and doc that names a `crates/<name>` path.
   Rename `frontend-oxc` to `kiln-frontend-oxc` in the same change. Land it fast.

## Acceptance

- The allow list in 05 is empty and deleted.
- Touching `crates/interp/kiln-interpreter/src/*.rs` does not rebuild `kiln-backend-c`
  (`mbx build -p kiln-native` after an interpreter edit recompiles no native-layer crate).
- `mise run gate` passes.
