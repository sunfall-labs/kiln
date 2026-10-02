# 08. Tier 3: worker arenas and allocation

Status: resolved
Blocked by: 06, 07, realms-and-concurrency issue 10

ADR 0069's worker arenas. The collector gains a paused state for the duration of a parallel
section, each worker allocates from private chunks of the kind the nursery uses, and at the end the
realm thread links every chunk into the heap's block table in one step. A worker that exhausts its
arena budget stops, and the call finishes sequentially from that element.

Callbacks may then return new objects (including object literals and spreads of plain data),
arrays and strings. Shapes created on a worker are a question to settle here: either shape
transitions are precomputed on the realm thread from the callback's literal shapes, or workers
create them privately and the join deduplicates them.

A human reviews the design before implementation: it is the largest change to the collector since
the nursery.

## Acceptance

- `rows.parallelMap(r => ({ ...r, total: r.qty * r.price }))` runs in parallel and produces objects
  indistinguishable from `map`'s, including their shapes' property order.
- Every tier 3 probe clean under `mise run native-stress`; no leaks across 10,000 repeated calls.

## Note, 2026-09-26

Joining arenas to the heap is ADR 0070's ownership transfer
(`docs/issues/realms-and-concurrency/issues/10-ownership-transfer.md`, step 2). Build the join there.

## Comments

### 2026-09-27 -- tier 3, branch `work/parallel-tier3`

The design this note asked a human to review first was implemented directly, on instruction; it is
laid out here so the review can happen against the code. The collector change is small and
local: a paused state, and a way to adopt chunks, both in `runtime/heap.c`.

**Worker arenas** (`heap.c`, "Worker arenas"). A tier-3 site opens a table of arenas on the realm
thread just before the pool (`kiln_c99_par_arenas_open`: one slot per pool thread plus the calling
thread, sized by the new provider export `kiln_parallel_threads`) and sets the heap's
`parallel_paused`. Each thread that runs a chunk claims one slot for the whole call
(`kiln_c99_par_arena_claim`, found again at its next chunk by the address of its own
`kiln_c99_no_write_depth`) and bumps through private chunks of the nursery's own kind
(`kiln_c99_par_alloc`): same region size and alignment, same header, allocation bit, and
finaliser bit for a kind finalised from birth. Nothing in the realm's heap changes while the call
runs; a pool thread never reaches the realm's allocator (the clone calls only the builders below),
and a collection that became due while `parallel_paused` is set aborts, like tier 2's collection on
a no-write thread. When the pool returns, the realm thread joins (`kiln_c99_par_arenas_join`):
every arena's last chunk is sealed and all chunks are adopted in one step (the ownership-transfer
primitive, realms issue 10 step 2), or freed when the call is answered sequentially.

**Budget and exhaustion.** An arena may take `KILN_PARALLEL_ARENA_KIB` of chunks (256 MiB by
default). Past it the worker declines like any tier-2 decline, and the call runs the sequential
method **from its first element**, with the arenas freed unadopted. Restart rather than resume, by
tier 2's argument: nothing the pool did is observable -- no user code ran, the realm's heap is
unchanged, and what the workers built is reachable from nothing -- so the sequential run is the
program's own run. Resuming at the failing index would need every sequential loop to start
mid-array over a prefilled result and a contiguous completed prefix (workers stop at chunk
boundaries, so the prefix below the lowest failure is not complete); a restart costs at most the
pool's share of one call that then runs sequentially anyway. `parallel-alloc-budget.js` runs with
a one-chunk budget: the large call is freed and answered sequentially, the small ones adopted.

**Shapes: found, or made the shared way; never realm-local on a worker.** A member definition on a
pool thread takes exactly the transition the sequential definition takes (`kiln_c99_par_define`:
same key, same double bit for the same value, so the same shape and property order). The edge is
looked up read-only (`kiln_c99_shape_edge_find`, split out of the transition code). A missing edge
between shared shapes by a shared key is created as any realm's thread creates one -- under the
process lock, published complete (ADR 0071) -- since that concurrency is what the shared tree was
built for. A missing realm-local edge (a key of the realm's own, or a realm-local parent) is not
created on a worker: the worker declines, the sequential restart makes the edge on the realm
thread, and every later call finds it. Precomputing literal shapes on the realm thread was
rejected: a shape depends on each value's kind (the double bit) and, for a spread, on the source's
shape, so the realm thread cannot know it without running the callback; per-worker private shapes
deduplicated at join would mean rewriting every built object's shape word at join, a per-object
step the join otherwise does not have. Constant member names are interned on the realm thread
before the pool (`kiln_c99_par_intern`, as the sequential definition would intern them); a
computed name is looked up and declines when the realm never interned it.

**What qualifies** (`parallel::heap_verdict`, rule 7). Object literals of at most 16 members with
plain constant names (not index-like, `@@`, `__proto__` or empty; computed names checked per
element), `{ ...r }` into a literal with nothing yet and the members defined after it, array
literals of at most 64 elements, `String(x)` (template substitutions) and `+` joining strings --
each built by `spread.c`'s tier-3 builders exactly as the sequential lowering builds it, or
declining (a spread source with accessors, a list-mode object, a function value, slots past the
inline sixteen, an existing double slot given a non-number). Closures and cells still refuse
(`allocates`). A string/object answer is now the site's question: `parallelMap`,
`parallelFilter` and `parallelFlatMap` keep it; `parallelReduce` and `parallelSort` refuse with
`returns-non-number` or the new `allocates-for-method`. Every heap site over an array now runs
its `_hmap`/`_hfilt` workers with arenas and keeps any answer (a tier-2 `r => r.name` included);
typed-array workers get no arena and decline at the first allocation (a building callback does not
try them). `parallelFlatMap` flattens on the realm thread after the join
(`kiln_c99_par_flatten`; a proxy answer declines). `--explain-parallel`: `parallel (tier 3: builds
values in worker arenas)`, marker `tier=3`. `KILN_PARALLEL_TRACE=1` prints each join.

**Acceptance.**
- `rows.parallelMap(r => ({ ...r, total: r.qty * r.price }))` runs in parallel and equals `map`
  element by element, keys in order (`parallel-alloc-objects.js`, with overwritten spread members,
  varying value kinds, computed keys, nested literals, a spread of `null`, answers that already
  exist, a spread of built rows, and a run-time getter that declines the pool with the getter
  called once).
- Stress, by hand: `native.sh` skips `parallelism=` probes under stress, so the new
  `parallel-alloc-stress.js` (no thread directive, allocation-free checks) is the one
  `mise run native-stress` runs; it matched at `KILN_GC_STRESS=1` at 1 and 4 threads. The five
  larger probes matched the interpreter at `KILN_GC_STRESS=64` (inputs cut to 1,500/3,000) at 1
  and 4 threads.
- No leaks: 4,000 enriched rows mapped (and filtered every seventh round) 1,000 and 10,000
  times: peak RSS 27,428 kB and 27,408 kB.

**Verified** (release `kiln` of this branch; logs under `target/tier3-logs/`):
- `mbx test -p kiln-backend-c --offline`: 206 + 36 passed, 0 failed (new
  `tier_3_builds_literals_spreads_and_strings`, `reason_allocates_for_method`,
  `tier_3_builds_run_in_worker_arenas_and_match_the_sequential_methods`; the tier-2 refusal test
  now refuses a closure and records a string answer; prelude band +~620 lines, ABI surface
  snapshot updated). Provider contract: 11 passed (ABI 49, 140 exports).
- `scripts/native.sh`: 289 matched, 1 refused (`shadow-realm`, pre-existing), 0 diverged, the six
  new probes at `parallelism=1,2,max` included.

**Bench** (`bench/parallel/run.sh --quick`, 1M elements, ms per call, load 7-11 on 16 threads;
before = release `kiln` at 5123a636):

| workload | node | bun | kiln seq | before @1 / @4 / @16 | after @1 / @4 / @16 |
|---|---|---|---|---|---|
| `enrich-records` `{ ...r, total }` | 348 | 27.5 | 396 | 394 / 393 / 396 | 398 / 63.0 / 53.0 |
| `flatmap-records` `r.qty > 4 ? [r.price, r.qty] : []` | 33.0 | 28.0 | 319 | 317 / 316 / 314 | 321 / 53.0 / 46.0 |
| `format-csv` `` `${r.id},${r.qty},${r.price}` `` | 194 | 146 | 409 | 426 / 427 / 430 | 417 / 123 / 93.0 |

The kiln sequential column is the thing to fix next: 11-14x bun on these rows, and the pool is
bounded by it (enrich @16 is 7.5x `map` but still 2x bun). Building in arenas is also bounded by
fresh memory: every chunk is new `malloc`'d memory, page-faulted on first touch, where `map`
reuses nursery chunks; handing the realm's spare chunks to the arenas is the obvious next step.

**Found here, filed:** issue 26 (the interpreter's `flatMap` keeps an answer's holes, so the
flatMap probe's answers are hole-free) and issue 27 (`native-release`: `i % 2 ? a : b` in a
two-parameter callback always takes `a`; the probes use `i & 1`).

**Risks / left.**
- A realm-local transition the realm has not made yet declines the first call (it then runs
  sequentially and makes it); objects whose keys the program never names as constants hit that.
- Shared transitions created by workers race only on who creates them first; a shape's fan-out
  ceiling (1,024 branches) could be reached by a different set of edges than a sequential run
  would reach, which would change which objects go to dictionary mode -- not what any program can
  observe (key order and values are the same), but not identical storage.
- A literal site's sequential cache can hand an object a learned shape whose double bit does not
  match the value (`kiln_c99_define_named_cached` does not compare it); a worker always takes the
  value's own edge, which is what an uncached sequential definition takes.
- Arenas do not reuse the realm's spare chunks (above), and a budget-exhausted call pays the
  pool's work twice.
