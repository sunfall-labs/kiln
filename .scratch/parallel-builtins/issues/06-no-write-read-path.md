# 06. The no-write read path

Status: ready-for-human
Blocked by: ADR 0069 accepted

ADR 0069's frozen heap. Reads on a worker must write nothing. Known writes on today's read paths:

- inline-cache learning (`kiln_c99_site_learn`, `runtime/read.c`);
- the cached hash in a string block's header (`runtime/heap.c`);
- key interning during lookup;
- lazily materialised intrinsic state.

Build a read path, generated from the same description as the ordinary one, that consults caches
without teaching them, computes hashes without storing them, and looks keys up without interning.
Before the pool starts, the realm thread materialises any lazy state the callback's proof says it
can reach.

This is the mechanism a human should review before work starts: it touches the collector's
contract and every hot read.

## Acceptance

- A test that runs the no-write read path under a write-detecting build (for example, the heap's
  pages mapped read-only for the duration of a parallel section) across the tier 2 probes.
- No measurable change to single-threaded read performance (`mise run bench`).

## Note, 2026-09-26

Built once, as the read path of ADR 0070's shared immutable region
(`.scratch/realms-and-concurrency/issues/08-shared-immutable-region.md`). This issue is done when
that path is usable from a parallel section.
