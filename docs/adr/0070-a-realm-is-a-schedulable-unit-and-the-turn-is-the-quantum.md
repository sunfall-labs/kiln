# ADR 0070: A realm is a schedulable unit, and the turn is the quantum

## Status

Accepted, 2026-09-26. Decided by the project owner.

Amended, 2026-09-28: Node-compatible workers with synchronous waits use dedicated threads;
async-only actor realms and eligible top-level turns use the bounded scheduler. The amendment
follows the worker wait liveness counterexample recorded under realms issue 09.

Originally proposed, 2026-09-26.

Supersedes ADR 0057 (region-isolated actors and zero-copy transfer): its goals stand, and its
mechanisms are replaced by the three primitives below. Builds on ADR 0040 (the executor is not the
realm state, accepted and not yet implemented), ADR 0041 (the collector runs at the turn boundary),
ADR 0028 (one Tokio runtime) and ADR 0038 (a realm owns its intrinsics). Gives ADR 0069's parallel
built-ins their pool, their read path and their arenas. Every new built-in it enables ships under
ADR 0068.

## Context

kiln has five concurrency mechanisms that share nothing:

- `KILN_WORKERS` forks one process per realm (`kiln-native/src/lib.rs`), because module namespaces
  live in a process-global array and two realms in one address space would read each other's
  values;
- `WorkerPool` in `kiln-runtime` hosts realms on threads, and nothing uses it;
- `parallelMap` runs proven-numeric callbacks on a separate Rayon pool (ADR 0058), which
  oversubscribes the machine when combined with `KILN_WORKERS`;
- `@kiln/concurrency`'s actors are in-realm proxies, and `isolate`/`transfer` are identity;
- `node:worker_threads` throws, and its `BroadcastChannel` and fallback `MessageChannel` silently
  do nothing.

The goal is one architecture that serves Node compatibility (`worker_threads`), server scaling,
data parallelism, sharing and actors, and that is not capped below what Go and Erlang reach.

Three facts in the code decide the shape:

1. **The heap is already reached through a realm context.** Generated code finds its heap through a
   thread-local cache that is valid only while the thread's realm generation matches
   (`kiln_c99_gc_heap`, `runtime/heap.c`). A thread can already host several realms in turn; the
   daemon does. What is still process-global is a smaller, countable set: about 45 mutable statics
   in the C runtime, about 57 kinds of static state in emitted code, and `kiln_namespaces`.
2. **The turn boundary is a safepoint with no generated frame on the stack** (ADR 0041). Everything
   a realm owns is reachable from its roots there, not from a thread's stack.
3. **ADR 0040 separates the realm's state from the executor that drives it**, for single-realm
   throughput. The same separation is what lets a realm be driven by any thread.

## Decision

**A realm is a schedulable unit. Its turns run on a shared set of scheduler threads, one per core,
and between turns a realm may move to any of them. Everything concurrent in kiln is built from
realms and three primitives: messages, a shared immutable region, and ownership transfer.**

This is the default for realms whose turns can relinquish their generated stack. A
Node-compatible worker may synchronously wait with that stack live; the exception below keeps
its owner thread available for the entire wait and preserves Node's result and progress rules.

### Realms

- A realm is single-threaded, deterministic and owns its heap, exactly as today. Nothing about the
  order of events inside one realm changes.
- **All realm state is reachable from the realm.** The pattern `kiln_c99_gc_heap` already uses is
  the only pattern: a thread-local slot is a cache of the current realm's state, validated by the
  realm generation, never the state itself. The runtime's statics, emitted code's statics and the
  module namespaces move into realm-owned storage behind that cache. On the hot path this is a
  thread-local load and a compare.
- **The turn is the quantum.** A realm runs one turn on one thread. Between turns it holds no
  generated frame and no thread-affine state, so the scheduler may run its next turn anywhere.
- **Pinning** keeps a realm on one thread when something in it is thread-affine: a loaded native
  addon, an FFI callback registered with a foreign library, or `Atomics.wait`. A pinned realm is
  the 1:1 model, which remains correct for every program.
- **A deterministic mode** runs every realm on one thread under the virtual clock, so tests and
  differential runs of multi-realm programs stay reproducible.

### The scheduler

One set of scheduler threads, sized to the machine's cores, runs eligible realm turns and the
chunks of parallel built-ins (ADR 0069) from one work-stealing queue. Tokio remains the I/O
reactor (ADR 0028); readiness wakes a realm, and its turn is queued on the scheduler. Parallel
chunks have no separate pool. A blocked lane may hand its movable work to a replacement lane;
its live synchronous stack remains on the original thread.

#### Amendment: a live synchronous stack needs its own thread

The bounded pool serves actors, parallel chunks and eligible top-level realm turns. Actors reject
synchronous `Atomics.wait` and use messages or `Atomics.waitAsync`, whose waiter wakes a realm
without holding a lane. Node-compatible `worker_threads` keep dedicated OS threads by default:
they must accept synchronous waits, including infinite waits, and return through the same nested
JavaScript and native frames. A blocked scheduled turn may start a replacement lane for movable
work, but that replacement cannot run pinned notifier turns owned by the occupied lane. Two
waiting parents with cross-routed pinned children form a deadlock under every finite fixed-lane
routing rule; issue 09 has a focused waiter-list counterexample.

Thus the cores-plus-reactor thread bound applies to async-only scheduled realms and parallel
chunks, not to Node worker graphs that may block synchronously. A future stackful, GC-visible
continuation could change this exception only after proving wait/notify progress, stack rooting,
TLS and cache reclamation, cancellation, termination, and memory bounds. The investigation in
`.scratch/realms-and-concurrency/worker-wait-continuation.md` records those gates. It is not a
prerequisite for actors or Node-compatible workers to work correctly today.

### The three primitives

- **A. Messages.** Values copied between realms with the structured clone algorithm, over ports.
  This is Node's `MessagePort`, `MessageChannel`, `BroadcastChannel` and `worker_threads`, with
  Node's semantics.
- **B. The shared immutable region.** Objects that can never change, readable by every realm and
  every scheduler thread without copying, through a read path that never writes (no inline-cache
  learning, no cached hashes, no interning). It first holds the built-in objects and the bootstrap
  heap, so a new realm does not rebuild or copy them; that is what makes a realm cheap. It then
  holds user data frozen into it.
- **C. Ownership transfer.** A group of objects moves from one realm's heap to another's by relinking
  its blocks, without copying. Its first user is Node's own transfer list
  (`postMessage(buffer, [buffer])`); its second is ADR 0069's worker arenas joining the heap.
  Moving an object graph requires a proof that the sender keeps no reference into it.

`SharedArrayBuffer` and `Atomics` keep their language semantics across realms: shared bytes, not
shared objects.

### What is built from them

| feature | built from |
| --- | --- |
| `node:worker_threads` | a dedicated realm thread when synchronous waits are possible, A, and C for transfer lists, with Node's semantics |
| server scaling (`KILN_WORKERS`) | N realms of the program in one process on the scheduler, sharing B; processes remain available for crash isolation |
| parallel built-ins (ADR 0069) | chunks on the scheduler over a realm blocked mid-turn, B's read path, and C to join worker arenas |
| immutable shared structs | B, with an API decided in its own ADR |
| actors | a realm per actor with A and C, as kiln built-ins (ADR 0068), with an API decided in its own ADR |

Shared *mutable* objects across realms are not offered. They would need a shared heap with a
concurrent collector and would put a synchronisation cost on every single-threaded access.

### How cheap a realm must be

The architecture's ceiling is set by the cost of a realm. The target is that a realm's own memory,
beyond the shared region, is measured in tens of kilobytes at creation and grows only with what it
allocates. At that cost, a realm per actor, per connection or per tenant is practical, which is the
granularity Erlang processes and goroutines give. The first measurement (plan issue 01) records the
starting point.

The starting point, measured at `ba4d1890` on Linux x86-64: a realm costs about 1.2 MB of private
memory for a hello-world program and 2.3 MB for the ten-line HTTP server, on top of 7-10 MB of
file-backed code that is already shared, and about 1.1 ms of the 1.5-1.9 ms from process start to
the first line of user code. The bootstrap's content is small, 164-214 KB of collector objects plus
181-295 KB of shapes and interned keys, but it touches about 1.5 MB of nursery chunks and slabs
before its first collection. With the bootstrap shared, what is left is allocation granularity (a
256 KiB nursery chunk and a 128 KiB slab, about 0.4 MB) and the emitted inline-cache arrays, which
are declared at full size (1.1-3.1 MB of zero-filled static storage); both must shrink by an order
of magnitude to reach tens of kilobytes.

## Consequences

- One realm and message model replaces five. The separate Rayon pool, the fork-only scaling path and
  the identity `isolate`/`transfer` are replaced or absorbed; Node workers retain dedicated threads
  for synchronous stack and wait semantics.
- `worker_threads` becomes a first-class Node feature instead of a refusal, which the deploy plan
  needs (pino transports, piscina).
- Server scaling shares one copy of the built-ins across every realm instead of one per process.
- Each realm collects its own small heap at its own turn boundaries, so there is never a pause
  across the process, and collection scales with cores.
- ADR 0040 stops being only a throughput fix and becomes the first step of this architecture.
- A crash in native code takes down every realm in the process. Process mode remains for that, and
  it gains supervision.
- Within one realm nothing changes, so single-realm programs, Test262 and the Node corpora are
  unaffected by construction.

## Rejected alternatives

**Realms pinned 1:1 to threads, with realm state in `_Thread_local`.** Simpler, and a large part of
the same work. Rejected as the end state because it caps a process at one realm per core and makes
a realm as heavy as a thread, which rules out actor- and connection-level granularity. It survives
as pinning, the fallback for thread-affine realms.

**Processes only.** What Node's `cluster` does and what `KILN_WORKERS` does today. Correct and
isolated, but it copies the built-ins into every process, cannot share memory between realms, and
cannot provide `worker_threads`.

**A shared heap with threads, as Go and Java have.** The most flexible model, and incompatible with
single-threaded JavaScript semantics, with the turn-boundary collector (ADR 0041) and with
determinism. Its cost would fall on every single-threaded program to serve a few multithreaded
ones.

**Keep ADR 0057's separate mechanisms.** Each was reasonable alone. Together they were five models
of concurrency with no shared substrate.
