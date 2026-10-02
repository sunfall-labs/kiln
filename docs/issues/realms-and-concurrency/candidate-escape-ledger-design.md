# Candidate lifetime escape ledger

Status: implementation plan, 2026-09-29. Completes issue 10's incoming proof scaling.

## Authority and lifetime

An owned candidate group is the authority for its allocation manifest and possible incoming
heap owners. Record owners when a store may introduce a reference into a candidate; do not
record only factory-time stores. The record lasts until that candidate is reclaimed, loses
provenance or is detached. It is weak metadata, not a GC root.

Track possible owner blocks, rather than permanent escaped/not-escaped bits or addresses of
individual slots. At admission, enumerate the owner's current audited fields using the existing
incoming layout walker. A cleared/replaced field then permits retry. Tracking the owner also
survives array/object side-buffer resize without retaining freed slot addresses.

Owner records cannot authorize a dereference of reclaimed memory. Resolve each recorded owner
through the current heap indexes before reading its fields; remove records at reclamation or
prune dead records during GC. Address reuse may conservatively retain an extra owner, but must
never turn a stale address into proof of a live edge. Allocation generation can eliminate that
conservatism if measured churn warrants it.

The ledger does not replace C, machine, provider or realm-root enumeration. Those inventories
remain independent authorities. Process-shared layout/key publication keeps its existing
ownership rules. Unknown layouts and uninstrumented escape seams fail closed.

## Store authority

All heap pointer stores must reach an audited authority, including optimized generated code:

- captured/local/module cells;
- object inline slots, grown slots, dictionaries, indexed elements, prototypes and descriptors;
- array creation, append, indexed writes, bulk copies, sorting and side-buffer replacement;
- closure/upvalue creation, generator frames, promises, callbacks and deferred tasks;
- collection/proxy/host-wrapper creation and mutation;
- raw key, shape and resource edges whose inventory is distinct from tagged value fields.

New allocations require recording their initialized edges before publication, even when a
constructor uses direct stores or memcpy. Marking just general setters would miss these paths.
Numeric stores cannot introduce a pointer and need no candidate lookup. Clearing a previous
pointer does not need to delete its owner witness; current-field enumeration handles it.
Internal candidate-to-candidate stores do not add incoming owners.

Use one owner-recording helper for runtime and emitted fast paths. Keep metadata allocation
cold and skip candidate lookup when no candidate can exist in the realm. Ordinary GC headers
retain their size. A failed ledger allocation makes certification unproven; it cannot silently
lose an incoming edge.

## Delivery and proof

1. Introduce shared store authorities and weak owner records, retaining the full outside-heap
   scan as the admission authority.
2. Audit every enumerated incoming kind against all allocator, mutator, emitter and host paths.
   Use compile-time const field views where practical to reject unaudited direct mutation.
   Keep an explicit inventory for paths that must initialize through writable storage.
3. In verification mode compare ledger enumeration with the full scan on positive and negative
   cases. Cover escaped aliases moved to new holders after factory completion, clears/retries,
   side resize, bulk stores, GC reclamation/reuse, weak references and host/provider roots.
4. Only after coverage is complete make the ledger the production incoming-heap authority.
   Preserve the full scan as an oracle. Benchmark certification across candidate sizes and
   independently growing unrelated sender heaps; check ordinary single-realm store workloads.

A partial ledger is not candidate-scaled certification. Keep issue 10 open until store coverage,
matching API behavior, GC/realm checks and the scaling measurements prove the complete path.

## Implementation checkpoint (2026-09-29)

The working implementation retains full-scan admission. Cell, array/object value-slot and
bulk-copy stores record weak owner identities, including post-factory writes. Parallel workers
skip realm metadata; chunk adoption records initialized fields after the complete batch is
indexed on the realm thread. Publication uses the existing incoming layout walk through a
temporary pointer observer, whose false result prevents field-list short circuiting. Unknown
publication layouts invalidate ledger proof conservatively for that heap.

Decoupled emitted units keep numeric/immediate stores inline and route pointer stores to the
runtime-owned recorder. Map entry updates, Proxy fields, Promise reactions/settlers and bound
constructor captures are covered by the same recording authorities. Suspended-frame locals, spills and lifecycle fields now use the same authorities; successful
collection prunes dead witnesses and shrinks sparse tables. Full constructor/mutator/host coverage, oracle
mode and scaling measurements remain required before production admission can use the ledger.
