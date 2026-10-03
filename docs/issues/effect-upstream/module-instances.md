# Module instance generations for direct ESM execution

Vitest's `vi.resetModules()` and `vi.importActual()` need fresh module state in the same realm. Canonical source paths remain stable for resolution, `import.meta.url`, diagnostics and compiled-code caching. A generation identifies mutable instances, not another source copy or realm.

## Native ownership

The realm owns CLI host services, globals, intrinsics and the job queue. Source/CommonJS/JSON module state belongs to a generation: export/import cells, namespaces, evaluation status and promises, CommonJS contexts, and generated unit values. Emitted functions and immutable source metadata are shared. The host ABI carries an explicit instance context and unit-value capability; generated activations use that host rather than a process-wide current-generation variable.

Closures capture their instance host when created. Calls and dynamic imports rebind to that captured provenance, so a retained old closure continues to read its original dependencies after reset. Typed native entries also receive the host when reading instance globals; their signature requirements include the activation prologue's unit lookup.

Node builtin modules have a separate realm-owned graph. It initializes only needed providers, and source graphs alias its export cells and namespaces. Builtin closures capture this stable graph's host. A reset therefore preserves `process`, URL constructors, `node:v8` namespaces/default objects and builtin state, without retaining a superseded source graph through a builtin closure.

Each graph has a GC-managed external-owner token. Closures, namespaces and live-binding cells retain it; its visitor traces the graph's external value slots once per collection. The loader roots the current generation. Active calls and pending top-level evaluation/job work root their own generation until settlement. Otherwise an old generation is reachable only through language objects or work that still uses it. Releasing an unreachable token unregisters its roots and frees its external storage exactly once.

The original runner generation can remain alive while its async entry is active. Its namespace's survival during that entry is not proof that an inactive library generation leaks. The collection fixture loads a library in generation 2, replaces it with generation 3, drops generation 2's namespace, then checks collection while generation 3 remains active. A separate fixture retains an old namespace/closure while a suspended top-level import resumes, proving that reachable old state survives.

At realm teardown, owner callbacks are detached while the heap is alive. Graph records and registered roots remain valid through realm destruction, then external graph storage is freed. This ordering prevents the collector from calling a graph root visitor through freed launcher state.

## Runner capability and language behavior

The direct runner registers its canonical original test referrer with a narrow loader capability. Reset requests a fresh generation; `importActual` resolves an original specifier against that referrer to a compiled canonical target. Literal target retention is declared in the direct runner's closed-world graph. Ordinary imports retain normal caching, and unrelated module closures keep their own provenance.

Module namespaces remain immutable and expose live export cells with ECMAScript descriptor and SameValue semantics. A test spy must use an explicit authorized loader/mock policy; making ordinary namespaces writable would change the language.

## Evidence

- Original native Config and HMR test files: all 178 cases passed with complete inventories and clean exits in `.scratch/effect-native-final-reload-three/report.json`.
- `native_module_reset_keeps_old_namespace_and_closure_on_old_instance`: independent generations, stable builtin identity, retained old closure and suspended top-level import.
- `native_module_reset_collects_superseded_generation_while_current_stays_alive`: replaced library namespace collected under GC stress while the current generation stays usable.
- `native_graph_cycle_keeps_builtin_closure_and_namespace_bindings_live`: cyclic builtin linkage and closure ownership.
- Native namespace, tagged-template and VM probes also passed ordinary and stressed collection, with tagged-template/VM probes checked in concurrent realms.

These are focused ownership/reload checks, not a claim that the complete upstream suite passes. The immutable full-suite survey records its own binary and runner digests separately.
