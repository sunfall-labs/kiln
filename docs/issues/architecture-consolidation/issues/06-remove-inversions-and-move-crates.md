# 06. Remove the layer inversions and move crates into layer directories

Status: resolved

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

## Comments

2026-09-26: Implementation is complete: module execution lives in `kiln-module-eval`, neutral
analysis and retained instruction/function counts in `kiln-graph`, and the DB receives its
summary query from the native caller. No C-byte sizing is needed by liveness; an unnecessary
forwarding provider introduced during the extraction was removed in review. The two number crates
are absorbed into the provider contract. All 37 workspace packages now live under layer directories; parser naming, benchmark
driver placement, source inventories, fingerprints, scripts and documentation follow the move.

The layer test has no exceptions. Touching interpreter source and rebuilding `kiln-native`
recompiles no native crate. The full gate passes every step except Test262: after 119 chunks with
no count regressions, `staging/sm` aborts allocating memory under the existing cap. A narrow run
reproduces the same allocation abort with an independently built, clean `30c1d548` baseline.
At that checkpoint the issue remained open because its full-gate condition was not met; no baseline or
memory limit was relaxed. See [verification](../verification.md) for evidence.

Follow-up: the allocation abort came from running too many interpreter workers within the capped
process. The Test262 runner now bounds workers using its memory budget, available CPUs and existing
job/thread settings. The unchanged 4 GiB cap now completes `staging/sm` with the same outcomes as
unchanged HEAD at equal concurrency. The final full gate passes all ten steps, including all 122
Test262 chunks, with no baseline edits. Evidence: `target/gate-logs/run.TgSsEtVC/aggregate.json`.
See [quality and resource review](../quality-and-resource-review.md).
