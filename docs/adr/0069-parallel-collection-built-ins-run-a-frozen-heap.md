# ADR 0069: Parallel collection built-ins run over a frozen heap

## Status

Proposed, 2026-09-26. The goals and the semantics are decided by the project owner: the parallel
built-ins should be extremely effective and powerful, and must never change Node behaviour
(ADR 0068), so a callback with side effects always runs sequentially. The mechanism (the frozen
heap, the no-write read path and worker arenas) is proposed for review.

Extends ADR 0058 (application 2: `parallelMap` over a Rust fork-join pool). Makes concrete the
region-transfer idea of ADR 0057 in the one place it is needed now: returning values that workers
allocated. Relies on ADR 0041 (the collector runs at safepoints) and ADR 0061 (the value graph is
built from bytecode, so a proof about a callback is a proof about the program).

## Context

`parallelMap` exists on arrays and typed arrays in both engines. The native backend runs a
callback across the pool only when `crates/kiln-backend-c/src/parallel.rs` proves it takes
numbers, returns a number, and touches nothing else: no captured variable (not even a constant),
no property or element read, no allocation, no call other than a listed `Math` function.
Everything else runs sequentially. The rules are carefully written and tested, but almost no
callback a developer writes in a real program qualifies, so the feature is a benchmark rather than
a capability.

The limit is the collector, not the proof. There is one heap per realm, with one table of every
block, and the collector assumes it is the only thread touching memory. What makes more possible
is that `parallelMap` is synchronous: the realm thread is blocked inside the call until it
returns, so for the duration of the call the heap cannot change unless the workers change it.

Reads are not as read-only as they look. A property read can teach an inline cache
(`kiln_c99_site_learn` in `runtime/read.c`), a string block caches its hash in its header
(`runtime/heap.c`), a key lookup can intern, and some intrinsic state materialises on first use.
Each is a write, and each is a race once two threads read.

## Decision

**A parallel built-in runs its callback across the pool over a heap frozen for the duration of the
call. Workers read the frozen heap without writing to it and allocate into private arenas that join
the heap when the call returns. A callback runs in parallel only when the compiler proves it has no
observable side effects; otherwise the call runs sequentially, with identical results.**

### Semantics

- The result, the order of elements, the exception thrown (the one from the lowest index that
  throws), and every observable effect are exactly those of the sequential method: `map` for
  `parallelMap`, `filter` for `parallelFilter`, and so on (ADR 0068 rule 3).
- For a method with no sequential equivalent in the language (`parallelReduce`, whose combiner is
  applied as a tree), the tree's shape is fixed by the input length alone, never by core count or
  scheduling, and the interpreter applies the same tree. The answer is deterministic and identical
  in both engines.
- The interpreter always runs the sequential form. It is the reference the native result is held
  to.

### What a callback may do and still run in parallel

The proof is extended in tiers. Each tier adds to what qualifies, and each is a separate change
with its own tests:

1. **Read captured values** that are never written after the callback is created.
2. **Read the heap**: arrays, typed arrays, object properties, strings, through a read path that
   writes nothing (below).
3. **Allocate**: return new objects, arrays and strings, through worker arenas (below).
4. **Call** functions the compiler proves pure, and built-ins marked pure in the member description
   (ADR 0059): string methods, `Math`, number formatting, `JSON.stringify` of plain data.

A callback that writes anything reachable from outside itself, performs I/O, reads the clock or
entropy, or calls something not proven pure, runs sequentially. There is no relaxed mode.

### The frozen heap

On entry, the realm materialises any lazy state the callback could reach and puts the collector in
a paused state; the realm thread then blocks on the pool. Workers read through a **no-write read
path**: inline caches are consulted but never taught, string hashes are computed rather than cached,
and key lookups never intern. That read path is generated from the same description as the ordinary
one, so the two cannot disagree about an answer.

### Worker arenas

Each worker allocates into private chunks of the same kind the nursery uses. They are invisible to
the collector while the call runs, because the collector is paused and no other thread can reach
them. When every worker finishes, the realm thread links the chunks into the heap's block table in
one step and the result array points into them. A worker that runs out of its arena budget stops and
the call finishes sequentially from that point, so memory pressure never becomes a failure.

### When to go parallel

A cost model decides per call: below a threshold of elements and estimated work per element, the
call runs sequentially, so a small `parallelMap` is never slower than `map`. Chunk sizes adapt to
the measured cost of the first chunk.

The pool is sized so kiln never oversubscribes the machine: by default, the available cores divided
by the number of `KILN_WORKERS` processes. `KILN_PARALLELISM` still overrides it.

### Saying why

`kiln build --explain-parallel` reports, for every parallel built-in call site, whether it runs in
parallel and, if not, the exact capture, write, read or call that prevented it. A performance
decision is reported as precisely as a refusal is (ADR 0063).

## Consequences

- Ordinary data-transformation callbacks (`rows.parallelMap(r => ({ ...r, total: r.qty * r.price }))`)
  run across every core, with results identical to `map`.
- The no-write read path and the worker arenas are general mechanisms. They are the foundation any
  later in-process parallelism would need (ADR 0057), built here for one bounded use.
- The collector gains a paused state and a way to adopt foreign chunks. Both need
  `mise run native-stress` coverage.
- Nothing about a parallel call is observable except its speed, so the extension adds capability
  without adding Node divergence.

## Rejected alternatives

**Run side-effecting callbacks in parallel with relaxed ordering.** Faster for more callbacks, and
it would make results depend on scheduling, which breaks ADR 0068 and makes the feature unsafe to
use without thinking.

**Share the heap across threads with locks.** Puts a cost on every single-threaded read and write
to serve an occasional parallel section, and conflicts with the collector's design (ADR 0041).

**Copy inputs to workers and copy results back.** Correct and simple, and it is `worker_threads`
with extra steps: the copying cost is what makes that model a poor fit for data-parallel work.
