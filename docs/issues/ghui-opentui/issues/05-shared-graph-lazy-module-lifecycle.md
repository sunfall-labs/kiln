# Shared graph wrapper must reuse the lazy module lifecycle

Status: planned

## Current limitation

`graph_launcher_source_with_imports` emits per-module evaluators, graph-owned module states, a
recursive static dependency ensure path, and a dynamic completion callback. It evaluates static
startup modules from `graph.statically_reachable()` and leaves dynamic-only modules untouched.

`graph_shared_wrapper_source` still allocates import/export arrays and namespaces, fills every
static import eagerly, and calls every emitted module in array order. It has no per-module phase,
active promise, startup reachability list, or async drive/re-entry protocol. Consequently a shared
graph can execute a dynamic-only module during wrapper startup and cannot correctly wait for a
top-level-await dependency reached by a dynamic import. This is a shared-wrapper limitation; all
modules must continue to be emitted and linked so a later import can resolve them.

## Design

Extract the Rust emitter currently owned by the launcher’s `emit_module_case` closure into one
storage-neutral `emit_graph_module_evaluator` function. Its configuration supplies the graph and
host expressions, module-state expression, import/export/namespace storage expressions, the
CommonJS context, the namespace-wrapper mode, and the optional entry-result slot. The existing
`graph_import_value`, namespace-plan, logical-path, and async-module decisions remain shared.

Emit the same C evaluator and recursive `ensure_module` for both launchers. The executable launcher
maps those expressions to its static arrays and stack graph context. The shared wrapper maps them
to arrays owned by its per-invocation realm context. The evaluator writes state and values but never
allocates or frees the arrays.

The shared realm context owns module states, active promises, import/export arrays, namespaces,
entry/result values, and the root-registration lifetime. Its proxy host keeps the caller’s callback
and context unchanged while carrying the graph context for generated dynamic-import completion.
Lifecycle state must not move to the existing TLS namespace/CommonJS aliases or any process-global
storage.

Startup uses `graph.statically_reachable()` in both modes. The shared body drives the realm while
the current startup module has an active promise, re-enters it after settlement, then performs the
final completion drive. Dynamic imports call the same graph-owned ensure path and return the cached
namespace on repeated imports.

## Acceptance

- Existing executable static-TLA regression remains green.
- A shared graph dynamically importing an unused module leaves its side effect at zero.
- Two concurrent imports of one shared target produce the same namespace and one evaluation.
- A shared target with a static TLA dependency waits for the dependency and returns its completed
  export.
- Shared host operations still reach the embedding host’s original context.
- The launcher and shared-wrapper lifecycle tests pass with `KILN_GC_STRESS=1`.
