# 11. Immutable shared structs: the API

Status: resolved (ADR 0073 implemented; program gate and collector stress passed)

Decide, in an ADR, how a program puts data into the shared immutable region and what it gets back:
the built-in (a kiln extension under ADR 0068), whether values are deeply frozen objects or a
distinct kind, how they cross `postMessage` (by reference, never copied), and how they relate to
TC39 proposals in the same space (Records and Tuples, Shared Structs) so kiln does not ship a
conflicting shape.

## Acceptance

- An accepted ADR, and the built-in implemented against 08.

## Comments

### 2026-09-27 -- API accepted

ADR 0073 defines `kiln:shared` as an explicit, deeply immutable snapshot with stable identity
across realms and shared-handle messages. The project owner delegated approval and implementation.
The built-in, graph arenas, message handles and reclamation are not implemented yet.

### 2026-09-28 -- implementation audit

The earlier implementation-pending note is stale. The `kiln:shared` module is registered in
both engines and exposes `share` and `isShared` through engine hooks. Sealed graph arenas,
realm-local facades, nested graph handles, queue retention, weak tokens and native ephemeron
tracing landed in `7dcb12c7` through `07a2478c`. Native probes cover aliases, cycles, holes,
descriptors, prototype views, invalid inputs, weak references, collector reachability and
cross-realm delivery (`shared-values`, `shared-weak`, `shared-gc`, and `shared-temp-root`).
Issue 08's realm-memory target is separate and remains open. The final program-wide gate and
collector stress run are still due before the program closes.

### 2026-09-27 -- Weak reachability follow-up

Shared graph facades now use GC-traced strong leases and generation-checked weak tokens.
Native WeakMap and WeakSet entries are ephemerons: the collector marks a value only after
another path has marked its key, repeats to a fixed point, then removes dead keys before
their blocks can be reused. A shared WeakRef keeps only an opaque weak token and resolves it
to the realm's canonical facade while another path retains the graph.

The worker compatibility work also added ordinary native WeakRef and FinalizationRegistry
constructors. Ordinary WeakRef still holds its target strongly, and FinalizationRegistry
records registrations without scheduling cleanup callbacks. Follow-up work should give
ordinary targets generation-checked weak cells, clear them before GC block reuse, and queue
cleanup jobs only after the target is unreachable; cover register, unregister, held values,
worker teardown and forced GC in both engines. Keep the pinned pino and piscina probes passing.
