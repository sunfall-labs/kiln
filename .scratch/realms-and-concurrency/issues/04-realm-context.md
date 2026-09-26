# 04. All realm state reachable from the realm

Status: ready-for-agent
Blocked by: 01

ADR 0070: a thread-local slot is only ever a cache of the current realm's state, validated by the
realm generation, as `kiln_c99_gc_heap` in `runtime/heap.c` already is. Apply that one pattern to
everything 01 classifies as realm state:

- the C runtime's mutable statics move into a realm-owned block, reached through the same cached,
  generation-checked pointer as the heap;
- the emitter's per-module statics (module slots, namespaces, inline-cache sites) move into a
  per-realm, per-unit block allocated on first entry;
- `kiln_namespaces` becomes per realm, removing the reason `KILN_WORKERS` had to fork;
- Rust statics in the runtime and provider that hold realm state move into `RealmState`.

A function loads the realm pointer once on entry, not at each access, where the emitter can.

## Acceptance

- The inventory from 01 shows no realm state outside the realm-validated cache.
- The small-service benchmark and `mise run bench` within noise of before.
- `mise run native-stress` clean.
