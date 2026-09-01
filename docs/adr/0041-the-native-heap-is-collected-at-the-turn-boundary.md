# ADR 0041: The native heap is collected at the turn boundary

## Status

Accepted. Implements the first collector slice that ADR 0021 deferred, and refines ADR 0021
decision 3 by choosing a safepoint that already exists over one that has to be emitted.

## Context

A natively compiled `node:http` server leaks between 7 and 12 KiB per request. Measured on
2026-09-01 with `bench/server/serve.mjs`: resident size grows from 3 MiB to 733 MiB over eighty
thousand requests. The cause is not a bug in any one place. The emitted C prelude has 87
allocation sites that produce JavaScript heap memory, and exactly two of them (`kiln_c99_new_object`
and `kiln_c99_make_closure`) register with the realm at all. What they register with is an
append-only vector of pointer/destructor pairs, `heap_allocations` in the runtime archive, that is
drained once, at realm destruction. Strings, arrays, cells, promises, bound functions, collections,
typed arrays, node sockets and responses are allocated with `malloc` and never freed.

ADR 0021 settled the shape of the eventual collector: frame chains and safepoints first, then a
narrow collector interface, then a conservative off-the-shelf collector. Its frame chain has not
been emitted, so there is no root set for any collector to start from. A server cannot wait for
that sequence: it needs bounded memory now, on the workload it has.

The native drive loop has one property the interpreter path does not: between two work items,
no generated C frame is on the stack. `drive` in the runtime archive pops a next-tick, a
microtask, an HTTP event or a timer, runs it to completion through `run_in_runtime`, and returns
to the top of its loop. At that point every live JavaScript value is reachable from a small set
of places that are not the C stack: module-scope slot arrays, per-unit intrinsic tables, the
symbol registry, the launcher's export/import/namespace arrays, and the Rust runtime's own
tables of queued work, pending continuations, attached HTTP connections, callable stores and
realm value slots.

## Decision

### Collection happens only at the top of the drive loop

The realm collects between work items and nowhere else. No safepoint is polled inside generated
code. A synchronous body that allocates without yielding is not collected until it returns, which
is exactly today's behaviour for that body and is documented as the limit of this slice. A server
yields at least once per request, so a server is bounded.

Decision 3 of ADR 0021 stands as the next step: when frame chains are emitted, the same collector
gains mid-turn safepoints and this restriction is lifted. Nothing here has to be undone for that.

### One allocator, one header, one table

Every allocation that a JavaScript value can point at, or that holds a JavaScript value, goes
through `kiln_c99_gc_alloc(kind, size)` in the prelude. The allocation carries a header naming its
kind, its mark epoch and its size, and is linked into a doubly linked list owned by a per-realm
heap. A hash set of block addresses answers "is this pointer one of ours" in constant time.

Inner growth buffers (an object's property array, an array's elements, a promise's reactions, a
typed buffer's bytes) stay `malloc`-backed and are released by their owner's finaliser. They are
never pointed at by a value, so they never need to be found from one.

The heap is one C struct shared by every translation unit. The prelude is emitted into each unit
with internal linkage, so each unit has its own copy of the collector's code; that is fine because
the copies are identical and operate on the one shared heap, which the first unit to allocate
creates and the runtime archive keeps. Tests that run the prelude without a realm get a static
per-unit heap and never collect.

### Marking never trusts a pointer

The tracer is precise about edges and paranoid about targets. For every candidate edge (a value
whose tag names a heap kind, a property key's byte pointer, a string's byte pointer, a hidden
host pointer) it looks the address up in the block table and marks only on a hit. A string value
may point at a unit's constant table, at a prelude literal, at a symbol's key bytes, or at Rust
memory during a callback; a symbol value may point at the well-known table; none of those are
blocks, and all of them are ignored by construction. The cost is one hash probe per edge. The
benefit is that an allocation site left on `malloc` degrades to today's leak rather than to a
use-after-free, and that no static or foreign pointer can ever be freed.

A property whose value carries the hidden-pointer sentinel (`len == 0xFFFFFFFF` on an object tag)
names a node struct, not an object. The tracer marks the block the pointer names and lets the
block's kind decide how to trace it.

### Roots are enumerated by whoever holds them

- Each translation unit registers a root function at realm entry. It marks that unit's module
  slot array, its intrinsic prototype table and its symbol registry.
- The launcher registers the export, import and namespace arrays it links with.
- The runtime archive enumerates its own tables when the collector asks: queued next-ticks,
  microtasks and timers, pending host-operation continuations, attached HTTP connection owners,
  callable stores and realm value slots. The archive never reads a C layout to do this. It hands
  each pointer or value slot to the marker hooks the prelude installed, and the block's own kind
  says what to trace.

Frame-transient prelude statics (`kiln_c99_new_target`, the argument window, the adopted `this`)
are not roots. They are meaningful only inside a call and are cleared at the turn boundary so a
stale read after a collection fails deterministically instead of dereferencing freed memory.

### Sweep runs finalisers by kind, and explicit release stays explicit

Unmarked blocks run their kind's finaliser (free inner buffers, release a provider handle) and
are unlinked and freed. Structs that today are freed by an explicit callback after use (queued
work payloads, host-operation completions) keep that explicit release, which unlinks the block in
constant time; the collector is the backstop, not a replacement for a known lifetime.

`WeakMap` and `WeakSet` remain strong in this slice, as ADR 0021 and the prelude already record.

### Policy and verification

A collection is triggered when bytes allocated since the last collection exceed the larger of a
fixed floor and the bytes that survived the last one. `KILN_GC_STRESS=1` collects at every turn
boundary; `KILN_GC_TRACE=1` prints each collection's counts to stderr. Stress mode under the
native corpus, the guard corpus and the server corpus is the acceptance test: a missed root
shows up there as a divergence or a crash, not in production.

## Consequences

- A native server's resident size becomes a function of live objects, not requests served. The
  bench that measures throughput also reports resident size after the run and must stay flat.
- Every allocation site that can hold or be a value is a `kiln_c99_gc_alloc` call, and every kind
  has a tracer and a finaliser in one place in the prelude. Adding a heap kind means adding it
  there, which is the locality this buys.
- The private realm ABI grows by two entries (attach the heap and its hooks; enumerate Rust
  roots) and drops the two ownership entries the vector needed. The public embedding ABI does
  not change. The provider ABI version and identity move, as ADR 0028 requires.
- The turn-boundary restriction is explicit. A CPU-bound synchronous program keeps today's
  memory behaviour until ADR 0021's frame chains land.

## Alternatives rejected

**A conservative off-the-shelf collector now.** ADR 0021 names it as the third step, after frame
chains. Without frame chains it would scan the C stack conservatively, which this slice does not
need at all, and it would ship a second allocator into a binary whose selling point is what it
does not contain. The vtable seam it would sit behind is what this ADR builds.

**Reference counting.** Sockets, requests and responses point at each other in cycles by design,
and the hidden-pointer back-links make every node object cyclic. Counting would leak precisely the
per-request garbage this exists to reclaim.

**A per-request arena.** JavaScript has no request scope; a closure created in one request may
be called in the next. An arena would have to be a collector in disguise to be correct.

**Trusting tagged pointers instead of a block table.** Faster per edge, but one un-converted
allocation site or one static pointer becomes a use-after-free instead of a leak. The hash probe
is the price of the fail-safe property, and it is paid only for live edges.
