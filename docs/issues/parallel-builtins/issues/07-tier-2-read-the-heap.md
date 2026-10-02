# 07. Tier 2: read the heap

Status: resolved
Blocked by: 05, 06

Callbacks may read arrays (dense and holey), typed arrays, own and inherited data properties of
ordinary objects, and strings (length, code units, comparison), through the no-write read path.
Anything that could run user code stays sequential: getters, proxies, `valueOf`/`toString`
coercions of objects, and exotic objects.

The proof needs to know a read cannot reach user code. Use the value graph's existing type facts,
and fall back to sequential when a receiver's kind is not proven.

## Acceptance

- `items.parallelMap(x => x.price * rates[x.currency])` runs in parallel when `rates` and the items
  are plain data.
- A getter anywhere on the path keeps it sequential, with the reason.
- Every tier 2 probe is clean under `mise run native-stress`.

## Comments

### 2026-09-27 -- tier 2, branch `work/parallel-tier2`

**What qualifies** (`parallel::heap_verdict`, tried whenever tier 1 refuses a callback or finds
it has no typed entry -- `parallel::tier_2`; every callback tier 1's source rules admit, tier 2's
admit too). Rules 1 and 2 are tier 1's. Values: numeric, boolean, `undefined`, `null` and string
literals, parameters and locals (loops included), phis, captures written once *of any kind*
(the realm thread reads them once, and refuses a binding still in its dead zone), every unary and
binary operator but `typeof`, `in` and `instanceof`. Reads: `o.name` and `o[key]`, `Math.<constant>`
(read as the constant). Calls: the listed `Math` functions and `String.prototype.charCodeAt`.
Every reachable `return` has a value, and none is proven a string or an object. Still refused, at
compile time: writes, allocation, any other call, `this`/`arguments`/globals, a reassigned
capture, and -- new, `reads-accessor` -- a name the module itself defines as an accessor (`get`/
`set` in an object literal or a class, found by `parallel::accessor_names` from
`DefineAccessor`), or `__proto__`.

**How it runs.** Not the typed `_p` clone: a tier-2 callback's values are boxed, so it gets a
*heap clone* (`_h`), emitted from its value graph by `parallel_heap.rs`, a lowering that knows only
what the verdict admits. Every read goes through the guarded readers of `prelude_types.h`/`read.c`
(`kiln_c99_par_get`, `_length`, `_index`, `_char_code_at`, `_relational`, `_strict_equal`,
`_loose_equal`, `_number`); an operator converts only what converts without running code. The
workers (`_hmap`, `_hmap_typed`, `_hfilt`, `_hfilt_typed`, `_hfold`, `_hfold_typed`, `_hcomb`,
`_hkey`, `_hrank`; `parallel_heap::HEAP_CHUNK_KINDS`) sit where tier 1's do, issue 14's optimised
worker unit included, and each enters no-write mode for its range (`kiln_c99_par_enter` =
`kiln_c99_no_write_enter` on the frozen heap) and keeps its own named-read cache (shape -> slot) on
its stack: nothing shared is taught. Reduce mixes tiers per callback (a tier-2 fold with a tier-1
combine, and so on).

**What could run user code, and where it is stopped.**
- *Compile time* (the value graph and the module): the refusals above.
- *Per call*, on the realm thread: kiln extensions on; the realm's built-ins unmodified (so no
  index member on `Array.prototype`/`Object.prototype`, `charCodeAt` is the built-in, and the
  only `Object.prototype` store members are the bootstrap's); the callback is the proven proto;
  captures past their dead zone; the receiver an array with no named properties or a Number-kind
  view that is not detached.
- *Per element*, on the worker: each read answers exactly what the ordinary read answers or
  declines. Only ordinary objects (`KILN_GC_OBJECT`, no `arguments`/error/namespace brand) along
  the whole chain; an own data slot of a shaped object or a list property; an accessor anywhere on
  the path, a refused member, a shaped object with a property list beside it, a prototype that is
  a function, a built-in or a proxy all decline. At the end of the chain a name is absent
  (`undefined`) unless it is `constructor`, `__proto__`, an `Object.prototype` method or a member of
  the realm's `Object.prototype` store, which decline. Arrays: an element, a hole or a key past the
  end reads `undefined`; views: a canonical numeric key reads the element or `undefined`; strings:
  `length`, code units, an index below U+0100 (a static one-unit string; above it would allocate),
  `charCodeAt`. A computed string key is looked up with `kiln_c99_interned_key_in` (never interns:
  a spelling the realm never interned names no property); an integer key on an object reads only
  its dense elements. Operators: numbers, `undefined`, `null` and booleans convert; strings compare
  by code units and `===` by value; a string `+` (allocates), an object's `valueOf` and a BigInt
  decline. Answers: `map` over an array keeps numbers, booleans, `undefined` and `null` (anything
  but a number marks the result not numeric), over a view anything `ToNumber` converts silently;
  `parallelReduce` numbers; `filter` anything (`ToBoolean`).

**A decline mid-call, and why re-running is `map` exactly.** A declining worker sets the call's
flag, every worker stops at its next chunk, and the site runs the method's sequential lowering
(issue 15's inline loop) *from the first element*. Nothing the pool did is observable: no user
code ran (every read that could have run some declined instead), nothing on the heap was written
(no-write mode; the write detector below checks it), and the only allocation -- the result, made
before the pool -- is dropped. So the sequential run is the program's own sequential run, and every
getter runs once, in order (`parallel-heap-getters.js` logs each call and compares the logs).
Resuming at the failing index would be equivalent too -- below it the pool's answers equal the
sequential ones, being the same pure reads of an unchanged heap -- but it needs every sequential
loop to start mid-array over a prefilled result; restarting costs at most the pool's share of one
call, on a call that then runs sequentially anyway.

**No collection during the call.** Nothing on any thread allocates: the heap clone has no
allocating operation, the readers are loads, the one-unit strings are static, and the realm thread
is inside `kiln_parallel_map`. The collector now checks it: an allocation that reaches a collection
on a thread in no-write mode aborts (`kiln_c99_gc_collect_mid_turn`), so `KILN_GC_STRESS` (which
collects at every allocation) turns any slip into a stop. The site allocates its result *before*
building the environment, so the pointers the workers read are the ones a collection there left
(the collector does not move blocks either).

**Key comparators** (`parallel::key_comparator`, carried in `HeapShape::key`): a straight-line
comparator returning `L - R` where `R` is `L`'s computation applied to the other parameter
(`(a, b) => a.k - b.k`, `b.k - a.k`, `rank[a.id] - rank[b.id]`, `a.p.x * 2 - b.p.x * 2`). Over keys
that are numbers and not NaN it is consistent exactly as `a - b` is, so a stable sort by key is
`sort`'s one answer. The pool computes every key (`_hkey`: a hole, `undefined`, a non-number or NaN
key declines), the provider's `kiln_parallel_sort` sorts a copy (descending negates), the workers
find each key's lower bound (`_hrank`), and the realm thread places the elements, equal keys in
their original order. The comparator is never called. No provider ABI change.

**`--explain-parallel`.** `parallel (tier 2: reads the heap)` (marker field `tier=2`); new code
`reads-accessor` ("reads `v`, which this module defines with a getter or setter"); the
`comparator-unproven` text names key comparators. `not-typed` is now reached only by a site whose
tier-1 verdict has no typed entry behind it: a callback the plan finds untyped runs as a heap clone.

**Acceptance.**
- `items.parallelMap(x => x.price * rates[x.currency])` with `rates` a dictionary-mode object and
  the items class instances: `parallel-heap-reads.js:40:16  parallelMap  parallel (tier 2: reads
  the heap)`, equal to `map` at 1, 2 and 16 threads.
- A getter on the path keeps it sequential: with the reason when the module defines it
  (`reads-accessor`, the class-getter and literal cases of `parallel-heap-getters.js`), and at run
  time when it cannot see it (`Object.defineProperty` on one element near the end, a proxy, an
  `arguments` object): the log of calls equals the sequential method's.
- Stress: `native.sh` skips `parallelism=` probes under `KILN_NATIVE_GC_STRESS` (by design, see the
  script), so the four tier-2 probes were run by hand: at `KILN_GC_STRESS=1` (every allocation
  collects; inputs cut to 4,001) at 1 and 4 threads, and at full size with `KILN_GC_STRESS=256` at
  16 threads -- all equal to the interpreter. With the collection check above, that is also the
  proof that no collection ran inside a section.

**Also here.** Issue 06's write detector (see its Comments): `KILN_PARALLEL_PROTECT=1` maps the
heap read-only around every tier-2 pool run; the four probes pass under it at 1, 2 and 16 threads.
New bench row `sort-records` (node, bun, pool, Go and kiln); `pipeline-records`/`-objects` lost
"(partial)".

**Verified** (release `kiln` of this branch, full logs under `target/tier2-logs/`):
- `mbx test -p kiln-backend-c --offline`: 199 + 36 passed, 0 failed. New: `tier_2_reads_by_name_by_index_and_compares_strings`, `tier_2_captures_of_any_kind_written_once`, `tier_2_still_refuses_writes_allocation_calls_and_this`, `reason_reads_accessor`, `a_key_comparator_is_a_difference_of_one_key_read_from_each_side` (`parallel.rs`); `tier_2_reads_run_in_the_pool_and_match_the_sequential_methods` (four-thread pool fixture: both sites take the pool, answers equal `map`/`filter`) and `a_callback_with_no_typed_entry_runs_as_a_tier_2_clone` (was `reason_not_typed`) in `parallel_tests.rs`; the write-detector test in `tests/objects.rs`. Prelude size band widened (+~480 lines) and the ABI surface snapshot updated.
- `scripts/native.sh`: 275 matched, 1 refused (`shadow-realm`, pre-existing), 0 diverged -- the four new probes at `parallelism=1,2,max` included.

**Bench** (`bench/parallel/run.sh --quick --only pipeline-records,pipeline-objects,sort-records`, 1M elements, ms per call, one round each, load 9-13 on 16 threads; before = release `kiln` at 6b8150ce):

| workload | node | Go (16) | kiln seq | before @1 / @4 / @16 | after @1 / @4 / @16 |
|---|---|---|---|---|---|
| `sort-records` `(a, b) => a.price - b.price` | 469-1,910 | 41-48 | 496-1,180 | 483 / 501 / 498 | 246 / 94 / 58 |
| `pipeline-records` (typed columns) | 41-43 | 0.37-0.39 | 36-38 | 24.2 / 24.5 / 28.5 | 24.2 / 8.1 / 5.4 |
| `pipeline-objects` (`{price, qty, rating}`) | 23-24 | 0.33-0.37 | 23-24 | 17.8 / 18.0 / 20.2 | 26.0 / 21.2 / 41.0 (noise, below) |

The quick run's `pipeline-objects` cells were one round under load; the same two binaries interleaved three times gave before 18.8-56 / 18.8-20 / 19.8-34 and after 24.5-62.5 / 12.4-14.3 / 12.5-13.5 (@1 / @4 / @16). The map stage alone (1M objects): `map` 21 ms, `parallelMap` @1 22.5, @4 10.9, @16 10.1 -- memory-bound: a kiln object is ~270 bytes (sixteen inline slots), so the scan moves far more memory than node's. At one thread the tier-2 pipeline runs ~25% behind the sequential one there (boxed clone and guards against `map`'s inline-cached loop); @4 and up it is ahead.

**Risks / left.**
- A decline restarts from the first element: a getter near the end of a large array costs up to one extra pool pass.
- The heap clone boxes every value; a callback whose tier-1 clone would take doubles still runs tier 1 (tier 2 is only tried when tier 1 refuses).
- `reads-accessor` is by name across the module: a getter `v` anywhere refuses every tier-2 read of `v`, including of plain objects (conservative, stated in the report).
- Heap writes the detector does not cover: large blocks, shapes, key tables (see issue 06).
- Key comparators are single-block only; `typeof`, `in`, string allocation, and string methods other than `charCodeAt` stay sequential (tiers 3-4).
