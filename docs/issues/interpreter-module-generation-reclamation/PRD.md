# Reclaim inactive interpreter module generations

Status: implemented; targeted validation passes. Full gate, native GC stress, and multi-realm validation remain pending.

## Problem

`ModuleMap` previously stored every source-module record in one strong map keyed by `(realm, generation, canonical path)`. `resetModules()` advanced the active generation but left prior records in that map so old closures and pending imports remained valid. The map itself therefore rooted all prior namespaces, export cells, failures, and exported values forever. Repeated resets could retain an unbounded amount of otherwise inactive source-module state.

## Ownership model

Represent one module generation as an identity-bearing `Rc<RefCell<ModuleGeneration>>`; the generation owns its records. The realm map strongly owns the current generation and keeps weak identities for inactive generations. Generation zero is deliberately also the static graph generation: static module closures retain the authority to import through epoch zero, so epoch zero remains rooted for the realm lifetime. Its dynamic-import records can therefore accumulate until that realm ends, just as records in any current or still-reachable generation can. Reset-created generations after zero are reclaimable when no closure, namespace, active frame, queued import, or suspended module body retains them. A captured generation handle, rather than a bare integer, travels with closures, frames, and queued dynamic-import jobs.

Add the generation to the existing cycle collector's owner graph. Its owner node traces record exports, stable binding cells, pending cycle cells, cached namespace values and failures. A closure has an edge to the generation it captured. Each generation-created namespace has a sidecar edge back to its generation, so retaining a namespace externally also retains its module record set. Record→exported closure→generation and generation→namespace→generation cycles remain collectible when no language or execution root reaches them.

Frames and queued jobs need no artificial roots: their strong generation handles appear as external references during collection, just like other live VM roots. A pending old import remains rooted until its job and promise/suspension release the handle. The collector clears mutable record edges only after proving the generation owner unreachable, then prunes expired weak-registry slots. Reset does not walk or eagerly clear prior records.

## Required scenarios

1. An old namespace retained across reset continues to expose its original bindings; after the namespace and its exports are dropped, explicit cycle collection reclaims the inactive generation and namespace.
2. A closure from an old module retains its generation after reset. The module-map lifecycle fixture verifies captured imports select that old handle while runner imports select the new current generation; the owner-graph fixture verifies an external closure retains its generation and that dropping it allows the unrooted cycle to be reclaimed.
3. A queued import handle and a top-level-await suspension each keep an old generation alive across reset and collection. Their lifecycle fixtures verify the handles are released when the queued/suspended owner is dropped; the upstream reset suites exercise actual import/reset completion.
4. The static generation and current generation remain rooted. Two realms do not share epoch identities or records.
5. Host CommonJS/builtin aliasing preserves canonical host cells, per-generation namespace wrappers, and cached snapshot/failure semantics under reset and collection.

## Collector invariants

- Every strong epoch reference stored in a GC-managed object is represented by an owner-graph edge; every active Rust frame/job reference remains an external root during collection.
- The weak epoch registry and weak cycle-census entries never root generations.
- All Values and cells owned by records are counted exactly once in the owner graph, including pending cells, failed evaluation values, cached CommonJS namespace failures, and namespace cells.
- Static and current-generation roots are explicit and survive even when no exported JS object refers to them.
- No record edge is cleared while a namespace, closure, frame, import job, or other modeled root retains the epoch.

## Validation

Focused owner-graph tests cover inactive-cycle reclamation, retained old namespaces and closures, queued imports, pending module awaits, and suspended-generator weak-census behavior. `mbx test -p kiln-interpreter --jobs=1` passed (254 passed, 0 failed, 1 ignored); `mbx test -p kiln-module-eval --jobs=1` passed (21 integration tests). The unchanged direct-TypeScript Effect files `Config`, `EffectKeepAlive`, `StackTraceLimit`, `rpc/RpcClient`, and `schema/HMR` passed 195/195 interpreter cases with zero Node/Vitest inventory, mode, or outcome mismatches. The report and raw logs are in `evidence/`. Native GC stress, native realm-count checks, and the full gate have not yet run for this milestone.

The Effect run used upstream commit `67ba4e46a11ccda0b6761578bfd22c04ae00167d`, runner commit `fa3a59dcd2f47733e9a7902dbf2b743709589034`, and frozen interpreter CLI SHA-256 `22af56585a882dca1cc9cfa9968f24c9c13be4b4fed286af58aa338fa5d32770`. The report captures the official Node 26.8.1 / Vitest 5.0.3 oracle, source digests, runner and mapping digests, case IDs, and results.

## Boundaries

This change does not implement Vite's transform policy for relative project `.cjs` files; the existing host loader models Node's external CommonJS cache. It also does not change `resetModules()` to invalidate static graph records. Generation lifetime must not be shortened merely to make tests pass: identity and old-closure semantics are the acceptance contract.
