# Module instance generations for direct ESM execution

Vitest's `vi.resetModules()` and `vi.importActual()` require fresh module state while retaining ordinary ESM behavior. A reset must create a new module instance graph for the affected loader/session without changing source identity, realm globals, host services, or normal import caching. The instance key is distinct from the canonical source path: resolution, `import.meta.url`, and diagnostics use the latter; module-map identity and export cells use the former.

## Current ownership

The interpreter has one `ModuleMap` per realm keyed by module path. `PlannedModule::key` serves both as source identity and state identity. Closures carry `module_path` for later dynamic imports, but not a module generation. A fresh generation therefore needs unique keys for every module in a plan and a closure-captured instance provenance for later `import()`; a session-wide active generation would make old closures import from the wrong graph.

The native launcher emits reusable functions for each source module. Each `kiln_graph_context` owns its module states and generated linkage fields (exports, imports, cells, namespace values, and CommonJS contexts), and its root visitor registers those values with the realm collector. `kiln_cli_realm_run` now owns the shared CLI/realm services separately and a graph context points to them. Dynamic imports and CommonJS resolution route through the graph pointer. Native closures currently carry lexical captures and diagnostic source but no graph-instance provenance; calls receive the current host, whose context identifies the active graph. Old exported closures would therefore require a retained generation context at invocation, not just a retained realm root.

## Required design

- Keep canonical source paths stable and cache parsed/lowered/emitted module code independently of instance identity.
- Give each requested reset a monotonically unique session generation and derive an instance key per canonical module path in that generation. Dependencies in one generation must resolve to that generation's providers; ordinary imports keep the current cache key.
- Keep realm globals, intrinsics, host adapters, job queue, and shared host services on the realm/session. Keep export/import/cell/namespace/loader status and evaluation promises on each graph instance.
- Capture module-instance provenance in closures created by generated modules. Invocation and dynamic import must use that closure's graph instance, while reading/writing realm-wide state through the shared realm record. An old closure must continue to observe its old imported cells after reset.
- Register every live instance's mutable value slots with GC roots. Release an instance only when its graph-owned values and closures can no longer reach it; never use one process-global active-generation pointer.
- Keep module namespaces immutable according to ESM semantics. Test mocking needs a separate Vitest runner/mock policy rather than making namespace objects mutable.

## Implementation sequence

1. Separate shared realm services from module-graph state without changing behavior. This is implemented in `Separate realm services from native module graph state`.
2. Make graph-owned linkage state addressable through an explicit instance pointer and add native tests that keep two instances live in one realm, checking independent cells, import routing, failures, and GC roots.
3. Add closure instance provenance to native calls and interpreter closures/dynamic-import jobs; verify a closure exported by generation A still reads A's dependencies after generation B starts.
4. Expose a narrow module-runner capability for `resetModules`/`importActual` that requests a new plan generation. Test same-realm global identity, changed module constructors/state, stable `Symbol.for` identity, and normal import cache behavior.

The first two ownership stages are now represented in the native launcher: realm services and an explicit `kiln_graph_module_instance` have separate records, but each realm run still constructs only one instance. This is groundwork only; it does not yet implement fresh instances. The native work is cross-layer because module-generated code, closure dispatch, host callbacks, and GC roots must all agree on instance ownership.
