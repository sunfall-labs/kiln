# Generational mid-turn collection

## Status

Proposal and implementation investigation. No collector policy has changed yet.

## Problem

The native Effect `Arbitrary` shrink workload is dominated by full-heap tracing during
allocation. A sampled batch-GDB profile of the exact `shrink` mode captured 16 interrupted stacks:
9 were in `kiln_c99_gc_collect` / `gc_trace` / `gc_drain`, 7 in mark routines, and 4 in object
property tracing. The uninstrumented run is about 13.5 seconds, against the upstream five-second
test timeout. A related cluster case also spends most of its budget in native object allocation
and marking.

The collector's opt-in heap profile measured 91,240 live blocks / 15.42 MB at the first turn
boundary and 93,923 / 15.82 MB after the shrink workload and follow-up turns. At the later sample,
the live graph contained 32,914 objects, 22,494 closures, 33,395 cells, and 3,413 arrays.
`internal/arbitrary/model.ts` accounted for 12,640 live closures (3.03 MB), almost all capturing.
Only three generator blocks and five promises remained. Thus nearly all live data is the stable
loaded module/function graph; the workload added about 2.7k live blocks after collection.
Across 16 CPU stack samples, 7 also touched `model.ts` generated function 36 / closure allocation.

The heap-local intern table contained 567 entries. Targeted removal of dead weak keys at sweep
instead of the existing table purge showed no measurable improvement (13.776 seconds before,
13.824 and 13.633 after); that experiment is not part of this proposal.

## Current collector behavior

Every allocation-pressure mid-turn collection calls `kiln_c99_gc_collect(heap, 1)`. The `force`
argument makes `tenured_due` true. The collector then enumerates roots, scans the C stack, traces
the complete reachable graph, and sweeps old blocks as well as nursery chunks. There is no
old-to-young remembered set or generational write barrier. Existing `kiln_c99_heap_store_*`
helpers only record cross-owner writes while actor-candidate provenance tracking is active; they
are not a complete GC barrier.

Skipping old-object tracing without a barrier is incorrect: an old object, closure cell, array,
promise, collection, or host node can be mutated to reference a newly allocated nursery value.

## Proposed invariant and collection model

Add a remembered-owner set for tenured heap blocks. Whenever a supported store publishes a heap
reference from a tenured owner to a nursery target, record the owner exactly once. At a minor
collection:

1. Enumerate the existing roots and scan the C stack at the same safepoint as today, but mark only
   nursery blocks reached directly from those roots.
2. Trace each remembered tenured owner in a restricted marking mode. Old blocks reached through
   these owners are not recursively traversed; their own new young references are covered by the
   same write barrier on the child owner.
3. Drain only reachable nursery blocks, settle/promote the current nursery exactly as the current
   collector does, and do not sweep or finalize old blocks.
4. Clear the remembered set after all reachable nursery blocks have either died or become tenured.
5. Run periodic full collections under the existing allocation/live-heap pressure policy to
   reclaim dead tenured blocks. The cadence must be derived from bytes allocated/promoted since the
   last full collection; do not change the existing live threshold, enlarge the nursery, or disable
   collection to make the workload pass.

Stress mode, collector fallback, candidate-transfer windows, and any case whose barrier coverage
is uncertain must retain the current full-collection behavior. Barrier allocation failure must
fall back to a full collection, never silently drop an owner.

## Barrier coverage to prove

Before enabling minor collection for a kind, audit its tracer edges and all mutation paths that
can create them. Initial coverage must include:

- ordinary object own properties, prototypes, boxed values, and error-stack values;
- closure upvalues, captured receiver/new-target/arguments, own prototype/store, source value,
  name, and external generation owner;
- cells and external-owner cells;
- array elements and properties/match fields;
- generator locals, active stack prefix, queued promises/sent values, delegate, receiver, function,
  and promise;
- promise result/properties/reaction slots;
- bound-call target, receiver, arguments, and properties;
- strong collection entries and properties;
- proxy target/handler, typed-array buffer/properties, data-view buffer, method-store links;
- regexp source/flags/properties/cached captures, symbols/descriptions, dates, and every
  `KILN_C99_HOST_LAYOUTS` value/pointer/array field;
- Rust/provider/module roots, queued jobs, weak-resource holdings, and C stack roots, which remain
  enumerated each collection and therefore do not require owner barriers.

Stores must call the barrier before publishing a new edge, and relocation within the same owner's
already-covered storage must preserve owner identity. Bulk copies, initialization of already-old
objects, deserialization, cloning, and host callbacks need explicit treatment. Kinds or edges that
cannot be audited remain on the full tracer path.

## Verification

- Add C unit fixtures for every edge family proving an old owner mutated to a young value keeps
  the value through a minor collection, then releases it after edge removal and a subsequent
  collection. Include cross-heap/foreign pointers, weak ephemerons, finalizers, symbol keys,
  generator suspension prefixes, external owner roots, and barrier-allocation failure fallback.
- Add native probes for repeated mutation of tenured objects/cells/arrays/collections and
  suspended `yield*` frames, checking Node/Bun observable results unchanged.
- Run `mise exec -- mbx test -p kiln-backend-c`, exact native `shrink` workload and the
  cluster workflow case; measure release binaries without GDB and report multiple runs.
- Run `scripts/native.sh` for affected probes, `mise run native-stress`, and
  `mise run native-realms` for representative probes (stressed realms as well) before claiming
  safety. Ordinary stress continues to use full collections.
- Compare full heap-profile kind/source/side-capacity counts, collection counts, freed bytes,
  and profile stacks before/after. A speed claim requires improvement on the exact workload with
  no broader allocation regression.

## Open questions

- Which central property/array/cell/host store functions cover all edges without duplicating
  barriers in callers?
- Can the existing GC block header carry a remembered bit without changing its layout or
  conflicting with current kind/mark flags? If not, which side-table gives bounded lookup and
  robust OOM fallback?
- What full-collection cadence reuses the existing allocation/live-byte policy while allowing
  dead tenured objects to be reclaimed proportionally?
- Which host-owned runtime structures can mutate independently of generated setters and thus must
  remain roots or conservatively force full collection?
