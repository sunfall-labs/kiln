# 15. Many concurrent tasks in one realm: time and memory

Status: resolved

The baseline's many-task row: 100,000 async tasks each doing 10 timer ticks take 2.9 s and 732 MB
peak RSS in one kiln realm, against node 1.2 s / 418 MB, bun 0.57 s / 196 MB and Go 0.30 s /
208 MB. Each pending task costs several kilobytes and each tick is slow.

Profile the timer table, promise reactions, async function frames and the nursery behaviour under
this load, and fix what dominates. This matters beyond benchmarks: a server with many idle
keep-alive connections or many scheduled jobs is exactly this shape, and ADR 0070's goal of a
realm per actor makes per-task cost inside a realm matter as much as per-realm cost.

## Acceptance

- The 100,000-task row at or below node's time and memory, natively; interpreter no worse than
  today.
- `mise run native-stress` clean on the probes added.

## Comments

### 2026-09-26 -- profiled and fixed (branch `work/many-tasks`)

**How it was measured.** `perf` (extracted package, `perf_event_paranoid=-1`) on an unstripped
`native-release` build of `bench/concurrency/js/tasks.mjs` (clang wrapper dropping
`-Wl,--strip-all`, isolated `.kiln/cache`): cycle and instruction sampling with frame pointers, a
page-fault profile for where resident memory is first touched, `KILN_GC_TRACE`, and gdb for
allocation call sites. The machine was shared and heavily loaded throughout (load 10-45 on 16
threads), so decisions were made on `cycles:u` minima and instruction counts over interleaved
runs, not on single wall times.

**What dominated, before.** 2.9-3.0 s and ~740 MB (7.1 KB per task above the 1,000-task run).

- *Memory:* 36% of all first-touch page faults were malloc'd side buffers, not GC blocks: an
  `arguments` object's property array (17%), promise reaction arrays (7%, 224 bytes for one
  reaction), async frames' four mallocs (6%: locals, stack and two handler arrays sized by the
  instruction count that no emitted body reads). The rest was nursery chunks, half of them
  dead space.
- *The `arguments` object.* Every arrow created inside a non-arrow function built that call's
  whole `arguments` object (an object in dictionary mode, a property array, four interned keys),
  whether or not the arrow could read it: `new Promise((resolve) => setTimeout(resolve, ms))` paid
  it on every sleep. It was ~40% of the garbage per tick and most of what a pending task cost.
- *The collector (~35% of time):* `kiln_c99_gc_lookup_nursery` 13%, sweep 5%, collect (settle)
  4%, marking. 900k live blocks re-marked at every collection, and every collection re-read the
  header of every block in every new chunk and every promoted chunk -- the living to see a mark,
  the dead to decide they needed nothing -- one cache miss each.
- *Timers (~9%):* `BinaryHeap::pop` over 100k entries (6.6%) and SipHash `HashSet<TimerId>`
  probes, a random cache miss per arm and per expiry (2.3%), plus an `Rc` malloc per timer.
- *Per pending task:* 9 GC blocks: the frame, its promise, `Promise.all`'s slot object, bound
  step, rejecting settler and unobservable derived promise, the sleep promise, its settler and a
  `kiln_timer` around it.

**What changed** (commits `62e4f6fb`, `365a3d2c`, `07801f73`):

1. Arrows that the `publishes_frame` fixpoint proves never read the frame are emitted as
   `arrow == 2`, and `kiln_c99_make_closure` captures no `arguments` for them.
2. Nursery chunks carry mark and finaliser bitmaps beside the allocation bitmap, and per-chunk
   allocated/marked counts. Settling a chunk is word arithmetic; the only header read is a dead
   block whose finaliser has work. Closures, promises and generators become finalisable only
   when a rare path gives them malloc'd storage (`kiln_c99_gc_note_finalizable`). Marks are
   cleared lazily per chunk (`mark_epoch`), so a fully live chunk settles in O(1).
3. Marking prefetches each candidate edge's header and looks it up 16 edges later (a FIFO drained
   before any mark is read). Lookups still validate every pointer (ADR 0041).
4. A promise's first reaction is inline; an async frame's locals and stack live in its block and
   the dead handler arrays are gone; `Promise.all` binds its step to (state, index), shares one
   rejecting settler and reacts without a derived promise.
5. `setTimeout(fn)` with a heap callback and no extra arguments hands the realm the function (a
   settler: its promise) with no `kiln_timer`; a timeout's owner lives inline in the timer entry
   (`NativeWork::Once`), only intervals share an `Rc`.
6. `kiln-runtime`'s timers are one FIFO run per delay and a heap of run fronts (Node's list per
   duration), exact `(deadline, sequence, id)` order kept by sending any out-of-order re-arm to a
   straggler heap -- unit-tested against a single heap over 20,000 random operations with late
   re-arms. Live timer ids hash sequentially (`SequenceHasher`). The native drive prefetches the
   payload of the timer four ahead; queueing an await's job prefetches its frame.
7. Promoted chunks that empty go back to the spares up to the live heap's size instead of being
   unmapped and faulted in again (page faults were ~10% of the run).
8. Found by `native-stress` on the way: caching a string's ASCII-ness rewrote its block kind and
   dropped `KILN_GC_FLAG_NURSERY`/`_LARGE` (a latent bug for `release_cell` too); only the ASCII
   bits change now.

Per pending task it is now 4 live GC blocks (frame, task promise, `Promise.all` step, sleep
promise), 570 B of payload.

**Before / after, 100,000 tasks x 10 ticks of 10 ms** (median of 5 interleaved rounds unless
noted; peak RSS is the process maximum; bytes per task = (RSS at 100,000 - RSS at 1,000) / 99,000):

| | wall | peak RSS | bytes/task | CPU |
| --- | --- | --- | --- | --- |
| kiln before (`fa213915`), results-2026-09-26 | 2,955 ms | 732 MB | ~7.2 KB | -- |
| kiln after, quieter window (load ~10, 5 runs) | 994-1,379 ms | 291 MB | 2.8 KB | 0.87-1.23 s |
| node, same window | 1,631-1,787 ms | 390-417 MB | 3.4-3.6 KB | 2.1-2.4 s |
| kiln before, load 43 | 12,698 ms | 724 MB | 7.2 KB | 7.8 s |
| kiln after, load 43 | 2,046 ms | 290 MB | 2.8 KB | 1.8 s |
| node, load 43 | 3,504 ms | 390 MB | 3.4 KB | 3.4 s |
| bun, load 43 | 1,986 ms | 174 MB | 1.4 KB | 1.8 s |

`bench/concurrency/run.sh --only tasks` (3 rounds, load 23-32): kiln ~1,687 ms / 291 MB, node
~6,440 ms / 386 MB, bun ~4,166 ms / 177 MB, Go ~377 ms / 168 MB -- node and bun suffered most from
the load there. In unloaded single runs kiln's in-program wall was 1.0-1.2 s. The 10 and 1,000
rows are unchanged (on the 100 ms floor); peak RSS at 1,000 tasks fell from 16 to 13 MB.

Interpreter: same checksums; `run` of 20,000 tasks is within noise in cycles (timer work fell
from ~9% to ~3% of instructions; total instructions +2.5% from inlining shifts elsewhere in the
interpreter crate). The virtual clock and timer order are untouched.

**Tests.** Crate tests: kiln-backend-c 176+36 pass (1 ignored), kiln-runtime 155, provider 33.
`scripts/native.sh`: 239 matched, 1 refused (shadow-realm, pre-existing), 0 diverged. Corpora
with the release CLI: `conformance/node --node` 36/36, `conformance/regress --node` 17/17,
`conformance/guard` 20/20. `native-stress`: a full run before the second commit reported 11
divergences; all 11 (and the new `many-tasks-timers` probe, also at `KILN_GC_STRESS=7`) match
interpreter output after it, re-checked probe by probe. The full stressed corpus was not re-run to
completion after the rebase: the `parallel-*` probes each run 30-50+ minutes under
every-allocation stress (on the base tree too), and the run was stopped at the user's request.

**Acceptance.** Time and memory are at or below node's in every interleaved comparison and the
interpreter is no worse. The stress criterion is met on every affected probe. A later stressed
run over the 229 non-`parallel-*` native probes matched 229, refused the existing shadow-realm
case, and diverged on none (recorded in realms issue 04's step 3c verification). The parallel
probes are excluded from that run because each can take tens of minutes under every-allocation
stress; none exercises this issue's task and timer changes.

**Left for later.** Nursery chunks are still promoted whole, so the heap is ~45% utilised with
100k tasks (promoted chunks hold 59 MB live in 172 MB); reusing dead space in promoted chunks
(Immix-style line recycling, now possible because nothing walks a chunk by headers) would cut
peak RSS by about a third and the page faults with it. A `kiln_closure` is 224 bytes and is the
largest piece of per-tick garbage. Under `KILN_GC_STRESS` the extra bitmaps cost one more page
per nearly-empty promoted chunk (about +25% RSS and up to +50% user time on
`json-shaped-build-edges`); ordinary runs are unaffected.
