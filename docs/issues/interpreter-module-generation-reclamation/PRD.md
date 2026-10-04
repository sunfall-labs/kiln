# Reclaim inactive interpreter module generations

Status: proposed

## Problem

`ModuleMap` currently stores every source-module record in a strong map keyed by `(realm, generation, canonical path)`. `resetModules()` advances the active generation but intentionally leaves prior records in that map so old closures and pending imports remain valid. The map itself therefore roots all prior namespaces, export cells, failures, and exported values forever. Repeated resets can retain an unbounded amount of source-module state.

## Ownership model

Represent one dynamic-import generation as an identity-bearing `Rc<RefCell<ModuleGeneration>>`. The generation owns its records. The realm map owns the current generation strongly and retains a weak registry for inactive generations. The static graph generation remains a strong root for the lifetime of the realm. Today the initial/static executor and generation-zero dynamic imports share the numeric epoch `0`; keeping that whole epoch as the static root would bound but not reclaim its dynamic records. Before implementation, either accept and document that one-generation retention or split static graph records from the initial dynamic cache while preserving their pre-reset identity. A captured generation handle, rather than a bare integer, is stored by a closure, active frame, and queued dynamic-import job.

Add the generation to the existing cycle collector's owner graph. Its owner node traces record exports, stable binding cells, pending cycle cells, cached namespace values and failures. A closure has an edge to the generation it captured. Each generation-created namespace has a sidecar edge back to its generation, so retaining a namespace externally also retains its module record set. Record→exported closure→generation and generation→namespace→generation cycles remain collectible when no language or execution root reaches them.

Frames and queued jobs need no special artificial roots: their strong generation handles are unexplained references to the epoch owner during collection, just as other live VM roots are. A pending old import remains rooted until its job and its promise/suspension complete. The collector clears mutable record edges only after proving the generation owner unreachable, then prunes expired weak registry slots. Reset does not walk or eagerly clear all prior records.

## Required scenarios

1. An old namespace retained across reset continues to expose its original bindings; after the namespace and its exports are dropped, explicit cycle collection can reclaim the inactive generation and its namespace.
2. A closure from an old module remains callable after reset and after collection. Its nested dynamic import resolves in the old generation. Once the closure is dropped, an export-cell/closure/epoch cycle with no other roots is reclaimed.
3. A pending import in an old generation survives reset and collection, then settles with the old generation's result. After settlement and release of its promise/namespace, the inactive generation becomes reclaimable.
4. The static generation and current generation remain rooted. Two realms do not share epoch identities or records.
5. Host CommonJS/builtin aliasing preserves canonical host cells, per-generation namespace wrappers, and cached snapshot/failure semantics under reset and collection.

## Collector invariants

- Every strong epoch reference stored in a GC-managed object is represented by an owner-graph edge; every active Rust frame/job reference remains an external root during collection.
- The weak epoch registry and weak cycle-census entries never root generations.
- All Values and cells owned by records are counted exactly once in the owner graph, including pending cells, failed evaluation values, cached CommonJS namespace failures, and namespace cells.
- Static and current-generation roots are explicit and survive even when no exported JS object refers to them.
- No record edge is cleared while a namespace, closure, frame, import job, or other modeled root retains the epoch.

## Validation

Add focused owner-graph tests for the three lifecycle scenarios above, including repeated GC after reset. Exercise normal and stressed interpreter collection; preserve the unchanged five-file reset-sensitive Effect run (`Config`, `EffectKeepAlive`, `StackTraceLimit`, `rpc/RpcClient`, `schema/HMR`) at 195/195. Run module-map and interpreter suites, realm isolation checks, and relevant GC regressions. The prior 195/195 evidence is in `docs/issues/effect-upstream/interpreter-module-generations.md` and the committed report at `.scratch/module-generation-validation-20261004/report.json` in the validation branch.

## Boundaries

This change does not implement Vite's transform policy for relative project `.cjs` files; the existing host loader models Node's external CommonJS cache. It also does not change `resetModules()` to invalidate static graph records. Generation lifetime must not be shortened merely to make tests pass: identity and old-closure semantics are the acceptance contract.
