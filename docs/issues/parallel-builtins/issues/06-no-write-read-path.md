# 06. The no-write read path

Status: resolved
Blocked by: realms-and-concurrency issue 08

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
(`docs/issues/realms-and-concurrency/issues/08-shared-immutable-region.md`). This issue is done when
that path is usable from a parallel section.

## Comments

### 2026-09-27 -- the path, built with realms issue 08 (branch `work/shared-region`, commit d044f96f)

**What it is.** Not a second read path: the ordinary one, with the thread in *no-write mode*
(`runtime/heap.c`, "The no-write read path"). Every read that would otherwise write does the
no-write half of its work instead, so the two cannot disagree about an answer:

| write on the read path | in no-write mode |
| --- | --- |
| inline-cache learning (`kiln_c99_site_remember`, which also registers a site with the heap) | consulted, never taught |
| a layout token issued to a list-mode prototype level (`kiln_c99_pic_name_level`) | not issued; the answer is found and not cached |
| the "all ASCII" answer cached in a string block's header (`kiln_c99_string_is_ascii`) | computed, not stored (key hashes are never stored anywhere) |
| key interning (`kiln_c99_intern_key`, `_pinned_key`, `_static_key`, `_shared_key`) | lookup only; an absent spelling answers NULL, which the caller treats as out of memory -- the section's signal to finish sequentially |
| a region closure's own prototype/store (realms issue 08) | read from the realm's table without making an entry |

**Entry points** (all in the prelude):

- `kiln_c99_no_write_enter(kiln_gc_heap *heap, kiln_c99_no_write_frame *frame)`: this thread reads
  `heap` -- the frozen realm's; `NULL` for the shared region alone -- and writes nothing, until
- `kiln_c99_no_write_leave(kiln_c99_no_write_frame *frame)`, which restores the thread's own realm
  cache. Nests.
- `kiln_c99_no_write()`: whether this thread is in no-write mode (one thread-local load).
- `kiln_c99_interned_key_in(heap, string)`: the lookup that never interns. A spelling neither the
  shared nor the realm table holds names no property of any object, so "absent" is the answer.
- `kiln_c99_in_region(pointer)`: whether a block is the shared region's (never marked, never written).

**What a parallel section still owes it** (ADR 0069): the realm thread materialises the lazy state the
callback's proof says it can reach (a function's first `prototype`, the global object, a built-in's
store) before the pool starts -- the path refuses to allocate only by the proof never admitting an
allocation, not by a check on the hot allocation path; and each worker must be online for issue 04
step 4's reclamation (`reclaim::online` in the provider) for the whole section, since it reads shared
cache slots.

**Write detection.** The shared region is `mprotect`ed read-only once sealed, so a write any path
forgot faults at the write. The realm's own heap is not protected during a section; a tier-2 test that
maps the frozen heap's pages read-only is the acceptance test above, and it needs the tier-2 probes.

**Verified.** `the_shared_region_holds_one_closure_and_each_realm_keeps_its_own_parts`
(kiln-backend-c) enters no-write mode on a second realm's thread reading the first realm's heap: the
string's header is unchanged after an ASCII question, a key never interned stays uninterned while an
interned one is found, a site learns nothing, a region closure's store reads through; after leaving,
the same calls cache, intern and learn. Disabling the mode fails the test. `mise run bench` was not
run; the added tests sit on miss, intern and caching paths only (one thread-local load each), none on
a cache hit.

**Left.** The acceptance test across the tier-2 probes (issue 07). Emitted caches that publish outside
`kiln_c99_site_remember` -- receiver rows, method paths, `lit_shape`, spread slots -- are write-once
CAS slots (issue 04 step 4), safe from any online thread, and are not switched off; if tier 2 wants
them quiet too, their learners are the next place to consult `kiln_c99_no_write()`.

### 2026-09-27 -- the write-detecting run across the tier-2 probes (branch `work/parallel-tier2`)

Acceptance item 1 now holds; item 2 (`mise run bench`) was not re-run here -- nothing in this
change touches an ordinary read (the tier-2 readers are separate functions, and the one line added
to the collector is on the collection path), so the path's hot reads are as issue 06 left them.

**The detector.** `KILN_PARALLEL_PROTECT=1` in the environment makes every tier-2 call site
(`kiln_c99_par_section`, `runtime/heap.c`) map the realm heap's slabs and nursery regions
read-only for the length of the pool's run and writable again after. Those are the pages holding
the headers of objects, arrays, strings, views and closures -- where a forgotten write on a read
path would land: a layout token issued, an ASCII answer cached in a string's header, a shape
changed. A write faults at the write. Not covered: large blocks (the allocator's, sharing pages
with other memory -- an array's out-of-block elements, a list-mode object's property array),
shapes, key tables and the emitted cache arrays, which the tier-2 readers never reach anyway
(they consult no inline cache and teach none). Off, a site pays one load. A map whose result lives
inside its own block (64 elements or fewer) is not protected, since the workers write it.

**Evidence.** `a_parallel_section_maps_the_heap_read_only_and_the_tier_2_readers_read_through_it`
(kiln-backend-c): under the detector, in no-write mode, the tier-2 readers answer an own, an
inherited and an absent name (twice through the worker's cache) and a string's length; a write
to the object's header in a child process dies with `SIGSEGV`; after the section the same write
succeeds. And the four tier-2 probes (`conformance/native/parallel-heap-*.js`: own, dictionary and
inherited reads, holes, typed columns, strings, key-comparator sorts, getters that decline) run
under `KILN_PARALLEL_PROTECT=1` at `KILN_PARALLELISM=1`, `2` and `16` with output identical to
the interpreter's. Tier 2 also makes the frozen heap's other half checkable: an allocation that
reaches a collection on a thread in no-write mode now aborts (issue 07's Comments).

Status left open for item 2 only.

### 2026-09-27 -- single-threaded read acceptance

The no-write path's effect on ordinary native reads was measured with paired release binaries:
`4e7d27c7` is immediately before the shared-region/no-write commit and `5653acdb` is immediately
after it. The benchmark sources are identical at those commits. Both CLIs and each of the five
native workload binaries were rebuilt from their own commit. Five interleaved A/B runs per row
produced the same checksum as Node:

| workload | before, warm median ms | after, warm median ms | after / before |
| --- | ---: | ---: | ---: |
| property-miss | 0.5859 | 0.5859 | 1.000 |
| class-poly | 1.8125 | 1.8125 | 1.000 |
| object-literal | 2.7188 | 2.6562 | 0.977 |
| object-spread | 0.2070 | 0.2070 | 1.000 |
| string-ops | 11.5000 | 11.3750 | 0.989 |

The host's 1-minute load average fell from 4.93 to 3.17 during the paired run. Raw records,
including all per-run medians and checksums, are in
[`read-path-2026-09-27.json`](../evidence/read-path-2026-09-27.json). No measured row regressed. The
write-detecting tier-2 run above and this paired comparison satisfy both acceptance items.

`mise run bench` also completed on current main: all 21 workload rows had a native answer with
Node's checksum, and the runner reported no divergences. That full sweep shared CPU with builds
and one interpreter `nbody` pass was paused while another quiet measurement ran, so its timings
are not the evidence for this issue's performance conclusion.
