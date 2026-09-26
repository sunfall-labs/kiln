# ADR 0066: Crates belong to layers, and dependencies point down

## Status

Proposed, 2026-09-26.

Builds on design.md §12 (foundational crates stay small and stable) and on ADR 0060 and ADR 0065,
which decide where semantics live; this ADR decides where the code that holds them lives.

## Context

The workspace has 37 crates in one flat `crates/` directory. Nothing records which crate may depend
on which, so the answer has been whatever compiled on the day. Reading the normal (non-dev)
dependency edges from `cargo metadata` on 2026-09-26 shows three that point the wrong way.

**The native backend depends on the interpreter.** `kiln-module-graph` does two jobs: it builds the
entry-rooted module graph, and it executes modules in the interpreter (`ModuleGraph::execute`,
`execute_node`, and about thirty imported interpreter items such as `ModuleRealmSession` and
`execute_module_in_runtime_with_loader`). `kiln-backend-c` needs only the first job
(`facts.rs` reads `ModuleGraph` and `ModuleSource`), and `kiln-liveness` likewise. Through them,
every native build compiles and links `kiln-interpreter` and `kiln-runtime`. An edit to the
interpreter's 50k-line `lib.rs` therefore rebuilds the native backend, which contradicts
design.md's second priority for no benefit.

**Liveness depends on the C backend.** `kiln-liveness` was moved out of `kiln-native` "so the
interpreter side can use it", but it reads `kiln_backend_c::facts::{DemandedMembers,
ModuleSizeFacts, method_groups}` and several `closed_world_*` functions. The interpreter side cannot
use it without depending on the C backend.

**The query engine depends on the C backend.** `kiln-db` calls `kiln_backend_c::summarize_program`
and `summaries_digest`. ADR 0001 put the engine behind a trait precisely so it would not know which
stages ran on it.

Separately, five crates carry the number provider (`kiln-number-core`, `kiln-native-number`,
`kiln-native-number-abi`, `kiln-native-provider-contract`, `kiln-native-provider-ffi`), a split no
document explains.

## Decision

**Every crate belongs to exactly one layer. A crate may depend only on crates in its own layer or
a lower one, and the two engine layers do not depend on each other.**

| layer | directory | holds | today's crates |
| --- | --- | --- | --- |
| 0 | `crates/foundation/` | vocabulary and pure algorithms everything shares, including the embedder seam's declarations | `kiln-string`, `kiln-digest`, `kiln-proto`, `kiln-data-formats`, `kiln-number-core`, `kiln-host-abi`, `kiln-artifact-cache` |
| 1 | `crates/semantics/` | language and platform meaning, engine-independent (ADR 0060 kinds 1, 2, 3 and 7), and declarations both engines read | `kiln-json`, `kiln-url`, `kiln-temporal`, `kiln-intl`, `kiln-node-path`, `kiln-module-resolver`, `kiln-builtin-surface`, `kiln-builtins`, `kiln-native-provider-contract`, `kiln-native-number`, `kiln-native-number-abi` |
| 2 | `crates/syntax/` | source to owned HIR | `frontend-oxc` (renamed `kiln-frontend-oxc`), `kiln-hir` |
| 2 | `crates/host/` | the realm's host runtime, shared by both engines | `kiln-runtime` |
| 3 | `crates/compile/` | HIR to bytecode, the module graph, facts about programs, the query engine | `kiln-ir`, `kiln-module-graph` (graph only), `kiln-liveness`, `kiln-db` (engine only) |
| 4a | `crates/interp/` | the bytecode VM and module evaluation | `kiln-interpreter`, and module evaluation moved out of `kiln-module-graph` |
| 4b | `crates/native/` | value graph, C emission, the C runtime, the provider implementation, linking | `kiln-backend-c`, `kiln-native`, `kiln-native-runtime`, `kiln-native-toolchain`, `kiln-native-provider-ffi` |
| 5 | `crates/tools/` | anything a person runs | `kiln-cli`, `kiln-daemon`, `kiln-conformance`, `kiln-test262`, `kiln-test-selector`, `kiln-bench` |

Directories are how a crate declares its layer; two directories may share one. Edges within a layer
are allowed except between `interp/` and `native/`.

`kiln-native-provider-contract` sits in layer 1 because it is a declaration both sides read:
`kiln-liveness` plans provider capabilities from it (ADR 0062) and the provider implements it. The
two number crates it depends on have no dependencies of their own and sit beside it until they are
consolidated.

With the three inversions below removed, every other normal edge in the workspace on 2026-09-26
already satisfies this table.

Dev-dependencies are exempt, so an engine's tests may still run the other engine differentially.

### The three inversions are removed

- Module *evaluation* moves from `kiln-module-graph` into layer 4a, as a module of
  `kiln-interpreter` or its own crate. `kiln-module-graph` keeps graph construction and closed-world
  reduction and depends on nothing above layer 3.
- The facts `kiln-liveness` reads (`DemandedMembers`, `ModuleSizeFacts`, method groups, the
  `closed_world_*` queries) move to layer 3 wherever they are backend-neutral. What is genuinely
  about C (sizes of emitted code) is supplied to liveness through a trait the native layer
  implements.
- `kiln-db` keeps the query engine. The concrete queries that call a backend register from the
  layer that owns the backend, so the engine has no dependency on any stage.

### The number-provider crates are consolidated

They are reduced to the splits that pay for themselves: the allocation-free core in layer 0, one
contract crate in layer 1 that absorbs `kiln-native-number` and `kiln-native-number-abi`, and the
provider implementation in layer 4b. A crate is kept separate only where that is shown to save
rebuild time or keep the staticlib build smaller.

### Enforcement

A workspace test reads `cargo metadata`, maps each crate to its layer from its directory, and fails
on a normal dependency that points up or across the two engines. The error names both crates, both
layers and this ADR. A new crate is placed by choosing its directory; there is no separate table to
keep in sync.

Each crate's `README.md` states its layer, what it owns, its invariants, and what does not belong in
it.

## Consequences

- An interpreter edit no longer rebuilds the native backend, and a C emitter edit no longer rebuilds
  the interpreter. Both are the common edits in this repository.
- "Where does this go?" has an answer that can be checked, which matters most when the person
  asking is an agent reading the tree for the first time.
- The move to layer directories is one large rename. It is done as a pure move in one change with
  no edits inside files, landed quickly because several sessions write to `main` at once, and
  `git log --follow` keeps history.
- Scripts and docs that name `crates/<name>` paths are updated in the same change. `mbx` and cargo
  address crates by package name and are unaffected.

## Rejected alternatives

**Enforce layers with no directory move.** A table of crate-to-layer keeps paths stable, but it is a
second list to keep in sync, and the flat directory keeps hiding the structure from anyone reading
the tree.

**Fewer, larger crates.** Merging along layers would make the rules trivial, but crate boundaries
are rustc's unit of parallel and incremental compilation. `kiln-interpreter` is already the slowest
crate to rebuild because it is one crate with one 50k-line file; the answer there is modules, and
where a boundary is real, more crates.

**`cargo-deny` or `cargo-hakari` for the check.** Neither expresses "no edge from layer N to layer
N+1" directly, and the workspace test is about forty lines with an error message written for this
repository.
