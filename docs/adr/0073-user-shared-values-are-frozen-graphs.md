# ADR 0073: User shared values are frozen graphs

## Status

Accepted, 2026-09-27, under the project owner's delegation to adjust or approve the API
contracts and implement them. The `kiln:shared` implementation subsequently landed in both
engines; the final program-wide gate remains due.

Specifies user data in ADR 0070's shared immutable region (realms issues 08 and 11). Follows ADR
0068 for kiln extensions, ADR 0060 for one semantic owner, ADR 0059 for realm intrinsics, and ADR
0071 for shared shapes and caches. ADR 0072's actor messages use the same shared-value rule.

## Context

The current region is a 4 MiB, process-lifetime reservation. The first realm builds the
bootstrap's closures in it, seals used pages with `mprotect`, and publishes them to later realms.
Its closures capture no realm heap pointers; their mutable property stores and prototypes are
per-realm. Ordinary bootstrap objects remain per-realm. There is no user-data allocation,
per-graph reclamation, or message token for an immutable object yet. Thus `Object.freeze(x)` is a
realm-local operation, not a way to send `x` by reference.

The API must make an explicit sharing request. An ordinary object passed to `postMessage` must
continue to be structured-cloned as Node specifies. A shared value must keep its identity and
contents when sent to another realm, without allowing a pointer from shared storage into either
realm's mutable heap.

## Decision

### Public API

The engine owns the kiln extension module `kiln:shared`:

```ts
import { share, isShared } from "kiln:shared";

const draft = { owner: "Ada", scores: [7, 9] };
const value = share(draft);
draft.scores[0] = 0;
// value.scores[0] is still 7.
port.postMessage(value); // receiver reads the same sealed graph
```

`share(value)` returns a new, deeply immutable snapshot of a supported object graph. It returns
a primitive unchanged. `share(value)` returns `value` itself when `value` is already shared.
`isShared(value)` is true exactly for objects produced by `share`, including their nested shared
objects. It is false for primitives and for ordinary `Object.freeze` results. A type declaration
offers a recursive readonly view and a brand, but runtime validation is authoritative; a TypeScript
cast never makes a value shared. There is no `#[]` or `#{}` syntax, `SharedStruct` constructor,
`Object.share` method, or implicit freeze.

`kiln:shared` is unavailable when `KILN_EXTENSIONS=0` or a binary is built with
`--no-kiln-extensions`. An import then produces the same named extension-unavailable error as
other kiln extension modules. Merely installing it changes no Node built-in, global or program
that uses only Node's surface. The engine module is the single definition; `@kiln/types`
declares it (2026-10-02: `@kiln/concurrency`, which re-exported it, is removed).

### Accepted graph and snapshot

A root may be `undefined`, `null`, a boolean, number, bigint, string, an ordinary array whose
prototype is this realm's `%Array.prototype%`, or an ordinary object whose prototype is this
realm's `%Object.prototype%` or `null`. Array holes,
non-index string properties, shared children, repeated references and cycles are supported.
Strings, including property names, are copied into shared storage where needed. A nested shared
graph is retained by the new graph without copying it; immutable graphs cannot subsequently form
a cycle across graph allocations. The result preserves the source graph's array lengths, holes,
own string-key order, aliases and cycles. Distinct calls on an ordinary source create distinct
identities, even if its contents are equal. There is no structural interning or value equality:
`share({ x: 1 }) !== share({ x: 1 })`.

Every reachable property must be an own **data** property with a string key. Enumerable and
non-enumerable properties are accepted and keep their enumerable flag; all resulting data
properties become non-writable and non-configurable. Arrays also receive a non-writable length.
Own symbol keys and accessor descriptors cause `TypeError`, including non-enumerable ones.
Functions, proxies, class instances, dates, regexps, errors, maps, sets, promises, weak
collections, typed arrays, `ArrayBuffer`, `SharedArrayBuffer`, host objects and objects with
private or other internal slots cause `TypeError`. A null-prototype record is accepted, but an
object with any other custom prototype is not. These restrictions avoid silently losing behavior
or leaving hidden mutable state in the sealed graph. Future graph kinds require an explicit ADR
amendment and both-engine parity.

Validation inspects object kinds and own descriptors without calling user code; in particular it
does not traverse a proxy or invoke a getter. It visits every reachable source node before
publication. The source realm is the sole mutator during the snapshot, so it sees a consistent
turn. On failure, `share` throws `TypeError`, publishes nothing and leaves the source unchanged.
The work and memory cost of the initial snapshot are proportional to the reachable graph; the
zero-copy promise applies to later sends, not to this conversion.

### Observable object semantics

Shared records and arrays have a distinct internal storage kind but present ordinary frozen
object and array behavior. They have object identity, not Record-and-Tuple value semantics. Property
reads, `Reflect.ownKeys`, `Object.keys`, descriptors, spread, `JSON.stringify`, array iteration,
`Array.isArray`, `Object.isFrozen` and `Object.isExtensible` follow ordinary frozen object/array
semantics. Ordinary write, delete, define-property and prototype-change operations fail as they
would on frozen objects (including strict-mode throws and `Reflect`'s boolean answer). Every
member is deeply shared; no mutable nested object is exposed. The only observable extra is
`isShared` and the cross-realm sharing rule.

The logical prototype of a shared ordinary record is the **reading realm's**
`%Object.prototype%`, or `null` when the source had a null prototype. A shared array's logical
prototype is the reading realm's `%Array.prototype%`. Its own properties remain sealed and
identical; inherited properties remain ordinary realm-local intrinsics, including modifications
made by that realm. This makes `instanceof Array` and generic array methods work in the receiver
without storing a realm-specific prototype pointer in the region. Prototype lookup is therefore
realm-relative, as it already is for region closures' mutable parts. The shared object itself
never points to a realm prototype. Within one realm its identity is stable, including when it is
sent away and echoed back. A pointer from two different realms is never exposed in a single
realm's object graph as two identities.

Shared objects may be `WeakMap`/`WeakSet` keys and `WeakRef` targets. Weak references do not keep
the shared graph alive; finalization becomes eligible only after the graph is unreachable from
all realms and message queues. No timing of a finalizer is promised beyond the usual JavaScript
rules. Implementations must not release a graph while a weak-key operation or finalizer check
still reads it.

### Messages and ownership

`postMessage`, actor calls, worker initialization data and `BroadcastChannel` encode a shared
graph as a retained region handle, with no traversal or copy of its data. Each receiver obtains
the same shared identity and reads its own realm-relative prototypes. A broadcast duplicates
only the handle. The same applies to `structuredClone(sharedValue)` in kiln: it returns the same
shared value. A shared object is **not transferable**: listing one in a transfer list raises
`DataCloneError` before publication. `SharedArrayBuffer` keeps its standard shared-bytes rule,
which is separate from this API. Ordinary frozen objects, arrays and all other Node values keep
their existing structured-clone and transfer behavior.

The message's retained handle is acquired before enqueue and released after delivery or queue
discard. A failed send releases it without changing the source. Encoding nested shared values
inside an otherwise ordinary object emits a handle at each shared edge and structured-clones
the rest. The token format carries an opaque graph ID and root offset, never a raw pointer
interpreted outside this process. A handle cannot cross a process boundary; a send that attempts
one raises a named `DataCloneError` before publication. It never silently copies a value the
caller explicitly made shared. Actor addresses and user shared values are separate kinds of
handle.

### Storage, reads and reclamation

Bootstrap closures keep their existing process-lifetime reservation. User calls allocate
**separate page-aligned graph arenas** with no fixed 4 MiB limit. Build and validate privately,
copying every accepted node into an arena whose edges lead only to the same arena, earlier sealed
user arenas, or immutable process-wide values. Seal all its pages before publishing the root with
release/acquire ordering. No collector traces or rewrites its blocks; it is not part of any
realm's heap table. The published arena never has an edge into a realm heap, including through a
shape, string, descriptor, prototype, closure, finalizer or cache. Shared shapes and keys obey
ADR 0071's publication and ownership rules.

The shared-object read path is the existing no-write path from issue 08: no cache teaching, hash
memoization, interning, lazy slot materialization or mutation of the sealed pages. Reflection and
prototype lookup must use that path too. A read of a realm-local inherited property uses that
realm's ordinary rules; parallel built-ins may do so only while their source realm's heap is
frozen under ADR 0069 and only when the callback proof excludes effectful getters or calls. A
normal property-cache entry can remember a shape and slot, never a graph pointer. Protected pages
provide a debug backstop, not the proof of immutability.

The realm collector traces shared handles as roots, but does not enter their arenas. Queued
messages and nested arenas hold their own strong references. When the final strong reference is
removed, the arena is retired; its pages are reclaimed only after every scheduler thread that
could have read it passes a turn boundary or the equivalent quiescent point for a pinned realm or
parallel chunk (ADR 0071). Cycles inside one arena are reclaimed together. Acyclic links between
arenas are released when their parent retires. Weak references do not count as strong handles.
This requires a process-wide reachability and retire protocol, not a permanent allocation per
call. `share` must refuse with an ordinary allocation error if its arena cannot be allocated;
it must never leave a half-published graph.

### Node adapter and standardization

A separate, explicitly documented Node adapter package could offer the same source-level
`share` call. On Node it makes an ordinary deeply frozen snapshot and supports `isShared` locally
with a `WeakSet`. A JavaScript-only adapter cannot reliably identify a `Proxy` without invoking
its traps, so it cannot promise kiln's exact validation or side-effect-free rejection. Node's
`postMessage` then performs its normal structured clone: it copies the graph and the receiver
does **not** retain shared identity or frozen status. The adapter must disclose these differences
and must not claim zero-copy sharing. Code that requires cross-realm identity or immutable
delivery detects kiln's engine module rather than assuming the Node adapter provides it. The
adapter does not monkey-patch Node prototypes or message APIs.

TC39's [Records and Tuples proposal](https://github.com/tc39/proposal-record-tuple) is withdrawn.
It proposed deeply immutable compound **primitives** compared by value; this API creates objects
compared by identity and does not reserve its syntax or global names. TC39's
[Structs proposal](https://github.com/tc39/proposal-structs) is at Stage 2 and includes mutable
shared structs and synchronization primitives. kiln's frozen snapshots have no mutable shared
fields, locks or class declarations. `kiln:shared` is a namespaced engine extension so a future
standard can be implemented independently, without treating this API as its polyfill.

## Implementation and acceptance

1. Declare `kiln:shared` once in the extension surface and provide the same validation and
   graph semantics through one shared semantic owner with engine-local heap adapters (ADR 0060).
   Remove the module with the existing extension switch.
2. Add user graph arenas, sealing, roots, cross-arena edges and deferred reclamation. Audit every
   accepted payload and read/reflect path for realm pointers and post-publication writes. The
   current bootstrap closure allocator alone cannot satisfy this step.
3. Add shared-handle message tokens and preserve them through structured clone, worker data,
   broadcast and actor calls. Preserve ordinary clone behavior for values not made by `share`.
4. Test identity, aliases, cycles, holes, descriptors, prototype behavior across realms, mutation
   attempts, invalid graphs, queue teardown, weak references and reclamation. Compare interpreter
   and native output, run the affected native realm probes with several realm counts, and run
   `mise run native-stress` because rooting and collection change.

These implementation steps landed in `7dcb12c7` through `07a2478c`. Neither the bootstrap
region alone nor `Object.freeze` implements this contract.

## Rejected alternatives

**Treat `Object.freeze` as sharing.** It changes Node's structured-clone behavior for a program
that never used a kiln extension, and a shallow freeze does not protect nested objects.

**Make mutable shared structs.** This needs atomics, synchronization and a concurrent collector;
it conflicts with ADR 0070's single-mutator realm and is a different problem from immutable
message data.

**Make records and tuples with value equality.** It would occupy the withdrawn proposal's syntax
and require new primitive equality semantics. Identity-preserving frozen objects meet this
program's sharing need without claiming a TC39 type.

**Keep every user graph until process exit.** The bootstrap's bounded reservation can do that.
Unbounded user input cannot.
