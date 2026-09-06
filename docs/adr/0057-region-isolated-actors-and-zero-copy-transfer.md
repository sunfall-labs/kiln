# ADR 0057: Region-Isolated Actors, Zero-Copy Heap Transfer, and Pure Data Parallelism

## Status

Proposed.

Extends ADR 0028's multi-realm runtime model and ADR 0041's turn-boundary garbage collector. Introduces structured in-process concurrency without shared-mutable heaps or structured-clone serialization.

## Context

Kiln today achieves multicore scaling through `KILN_WORKERS=auto`, which forks independent operating-system processes over a shared `SO_REUSEPORT` socket ([README.md](file://~/kiln/README.md#L68-L77)). Each process runs a single realm on a current-thread Tokio runtime (ADR 0028), serving ~111k requests/second on a single core with an 8–10 MB resident set. Interpreter multi-agent execution exists solely for Test262 conformance on a virtual-clock baton (ADR 0039).

When workloads require in-process coordination, stateful caches, background processing, or data-parallel transformations across CPU cores, modern runtimes present three unsatisfactory compromises:

1. **Web Workers and `node:worker_threads` (Actor model with structured clone):** Heaps are strictly isolated, but communicating non-trivial state requires deep serialization via `postMessage`. Passing a 50 MB object graph incurs tens of milliseconds of CPU serialization latency and doubles peak memory.
2. **Unconstrained shared heaps (e.g. Bun / WebKit PR #249 `new Thread(fn)`):** Arbitrary closures capture and mutate shared heap objects across OS threads. This introduces severe data races, corrupts shape transition trees (ADR 0050), breaks Secure ECMAScript (SES) boundaries, and forces atomic fences or locks onto every single-threaded property read and shape check. Crucially, it is completely incompatible with Kiln's turn-boundary mark-sweep GC (ADR 0041), which assumes no concurrent mutator runs while sweeping.
3. **TC39 Structs and Shared Structs (Stage 2):** Solves memory safety by restricting shared objects to fixed layouts (`shared struct`) and enforcing that shared objects cannot reference unshared objects. However, it regresses developer ergonomics to 1995-era manual locking (`Atomics.Mutex.lock()`, `Atomics.Condition`), introducing deadlock hazards and forcing developers to maintain two disjoint object models (`class` vs. `struct`).

### Grounding in Modern PL Research

Programming language research over the past decade provides superior models:

* **Pony's Deny Capabilities (OOPSLA 2015, 2017):** Proves that an isolated object reference (`iso`) with no external aliases can be transferred between actors via an $O(1)$ pointer handoff with zero copying, zero serialization, and zero locks.
* **Swift 6 Region-Based Isolation (Apple, 2024; SE-0414, SE-0430):** Demonstrates that isolated subgraphs do not require complex lifetime or capability syntax; the compiler can infer disconnected regions from control-flow escape analysis.
* **Project Verona (Microsoft Research, 2020–2022):** Eliminates per-object mutexes by grouping allocations into isolated regions where memory operations remain thread-local and lock-free.
* **Lean 4 / Perceus (ICFP 2020, PLDI 2021):** Shows that immutable data structures can achieve C-level performance through in-place destructive updates when reference counts are statically or dynamically proven unique.

## Decision

We introduce a three-tiered concurrency architecture for Kiln:

1. **Language-Level Native Actors:** Isolated state machines communicating via typed asynchronous mailboxes.
2. **Zero-Copy Region Transfer:** $O(1)$ pointer transfer of unaliased object subgraphs across realms, implemented by splicing Kiln's doubly-linked GC block chains.
3. **Compiler-Verified Pure Data Parallelism:** Parallel collection transformations (`parallelMap`) lowered directly to native work-stealing C loops.

Crucially, **all primitives must typecheck cleanly under standard, unpatched `tsc` (TypeScript Compiler)** using standard mapped types, decorators, and brand generics. No custom TypeScript syntax is introduced.

---

### 1. Developer Surface and `tsc` Compatibility

The concurrency API is exposed via `@kiln/concurrency` (built into the runtime, resolved via ambient types).

```typescript
import { Actor, type ActorRef, spawnActor, isolate, transfer } from "@kiln/concurrency";

// 1. Actors: Ordinary single-threaded JS/TS classes inside
export class ImageCache extends Actor {
  private store = new Map<string, Uint8Array>();

  async put(key: string, data: Uint8Array): Promise<number> {
    this.store.set(key, data);
    return this.store.size;
  }

  async get(key: string): Promise<Uint8Array | undefined> {
    return this.store.get(key);
  }
}

// 2. Consumption: Fully typed asynchronous proxy
const cache: ActorRef<ImageCache> = spawnActor(ImageCache);
const count: number = await cache.put("avatar", buffer);

// 3. Zero-Copy Region Transfer:
function buildLargeGraph(): SceneGraph {
  const scene = new SceneGraph();
  scene.populate();
  return scene;
}

// 'isolate' asserts unaliased region; 'transfer' passes ownership to another realm
const graph = isolate(buildLargeGraph());
await worker.send(transfer(graph));
// Static or runtime assertion: 'graph' is inaccessible in this realm after transfer
```

#### TypeScript Typing Contract (Valid under `tsc --strict`)

The ambient declarations in `crates/kiln-builtin-surface` define:

```typescript
declare module "@kiln/concurrency" {
  /**
   * Base class marking an Actor.
   * State within an Actor is strictly private and single-threaded.
   */
  export abstract class Actor {}

  /**
   * Transforms all methods of an Actor class into Promise-returning calls.
   */
  export type ActorRef<T extends Actor> = {
    readonly [K in keyof T as T[K] extends (...args: any[]) => any ? K : never]:
      T[K] extends (...args: infer P) => infer R
        ? (...args: P) => Promise<Awaited<R>>
        : never;
  };

  export function spawnActor<T extends Actor>(
    ctor: new (...args: any[]) => T,
    ...args: ConstructorParameters<new (...args: any[]) => T>
  ): ActorRef<T>;

  /**
   * Phantom brand denoting a disconnected, unaliased object subgraph.
   */
  declare const IsolatedBrand: unique symbol;
  export type Isolated<T> = T & { readonly [IsolatedBrand]: true };

  /**
   * Marks an expression as an isolated region.
   * In Kiln, triggers static escape analysis.
   */
  export function isolate<T>(value: T): Isolated<T>;

  /**
   * Consumes an isolated region and prepares it for O(1) cross-realm transfer.
   */
  export function transfer<T>(value: Isolated<T>): T;
}

declare global {
  interface Array<T> {
    /**
     * Executes a pure parallel map across worker threads.
     */
    parallelMap<U>(fn: (item: T, index: number) => U): Promise<U[]>;
  }
}
```

* **Compatibility:** Under standard `tsc`, `ActorRef<T>` uses standard mapped types with key remapping (TS 4.1+). `isolate` and `transfer` are standard generic identity functions with unique symbol brands. No Babel/Oxc parser plugins are needed.

#### Effect v4 Integration (`@kiln/concurrency/effect`)

For codebases adopting Effect v4 (`effect@4`, tested in `conformance/effect`), the same primitives map directly to Effect's structured concurrency, fibers, scopes, and layers:

```typescript
import { Effect, Context, Layer, Scope, Queue } from "effect";
import { spawnActorLayer, isolate, transfer, forEachParallel } from "@kiln/concurrency/effect";

// 1. Define an Actor Service with Effect v4 idioms
export class MeshService extends Context.Service<MeshService, {
  readonly process: (mesh: Uint8Array) => Effect.Effect<number>;
  readonly clear: () => Effect.Effect<void>;
}>()("MeshService") {}

// 2. Layer construction: Bound to an independent Tokio Realm
export const MeshServiceLive: Layer.Layer<MeshService, never, Scope.Scope> =
  spawnActorLayer(MeshService, () => {
    // Single-threaded state inside the Actor's Realm
    const cache = new Map<string, number>();

    return {
      process: (mesh) => Effect.sync(() => {
        cache.set("last", mesh.length);
        return mesh.length;
      }),
      clear: () => Effect.sync(() => {
        cache.clear();
      })
    };
  });

// 3. Multi-Core Data Parallelism via Effect Pipelines
export const computeAll = (items: string[]): Effect.Effect<string[]> =>
  forEachParallel(items, (item) => Effect.sync(() => heavyTransform(item)), {
    concurrency: "native"
  });

// 4. Zero-Copy Transfer over Effect Queues
export const sendMesh = (queue: Queue.Queue<Uint8Array>, raw: Uint8Array) =>
  Effect.gen(function* () {
    const isolatedMesh = yield* isolate(raw);
    yield* Queue.offer(queue, transfer(isolatedMesh)); // O(1) GC block list splice
  });
```

##### Effect v4 Ambient Typing Contract

```typescript
declare module "@kiln/concurrency/effect" {
  import type { Effect, Layer, Scope } from "effect";

  /**
   * Spawns an in-process Actor Layer backed by an independent Tokio Realm.
   * Lifecycle is tied to the enclosing Scope: closing the scope shuts down the realm.
   */
  export function spawnActorLayer<Id, Shape, Error = never, In = never>(
    service: { readonly key: string; new (...args: any[]): Id },
    factory: () => Shape
  ): Layer.Layer<Id, Error, In | Scope.Scope>;

  export function isolate<A>(value: A): Effect.Effect<import("@kiln/concurrency").Isolated<A>>;

  export function transfer<A>(value: import("@kiln/concurrency").Isolated<A>): A;

  /**
   * Evaluates effects across native Tokio worker threads using work-stealing.
   * Preserves Effect fiber interruption and error-short-circuiting semantics.
   */
  export function forEachParallel<A, B, E, R>(
    elements: Iterable<A>,
    f: (a: A) => Effect.Effect<B, E, R>,
    options?: { readonly concurrency?: "native" | number }
  ): Effect.Effect<B[], E, R>;
}
```

* **Lifetime Management:** Spawning an Actor as a `Layer` ties the lifetime of the underlying Tokio realm to an Effect `Scope`. When the scope closes, Kiln signals the actor's Tokio task to drain its queue and drop the realm, reclaiming all memory deterministically without relying on process exit.
* **Fiber Interruption:** When `forEachParallel` runs with `concurrency: "native"`, Kiln links Effect's fiber interruption token to atomic cancellation flags in the native C worker loop. If one task fails or the parent fiber is interrupted, pending chunks on other CPU cores are halted immediately.

---

### 2. Runtime Execution Model: Independent Tokio Realms (ADR 0028)

Each spawned `Actor` is backed by an independent `Realm` running on a Tokio thread:
* **Disjoint State:** Each Actor owns its own JavaScript globals, shape tables (ADR 0050), interned property keys (ADR 0047), and heap.
* **Turn-Boundary GC Preservation (ADR 0041):** Inside an Actor realm, GC collections happen strictly at turn boundaries (when the Actor's mailbox queue drains). There are no safepoints inside generated C code, and no cross-thread GC barriers.
* **Mailbox:** An Actor’s incoming calls are queued in a bounded Tokio `mpsc` channel. The realm processes envelopes sequentially, executing callbacks to completion and returning the resolution via a oneshot channel that completes the caller's native `Promise`.

---

### 3. Zero-Copy Heap Transfer via GC Chain Splicing

When `transfer(isolatedGraph)` is called, Kiln performs an **$O(1)$ pointer transfer** instead of serialization.

#### The Splicing Mechanism in `kiln-backend-c`

Under ADR 0041, every native heap allocation is represented by a `kiln_c99_gc_block` header embedded in a doubly linked list owned by the realm's heap:

```c
struct kiln_c99_gc_block {
    struct kiln_c99_gc_block* prev;
    struct kiln_c99_gc_block* next;
    uint32_t kind;
    uint32_t mark_epoch;
    size_t size;
};
```

1. **Isolation Verification:** `kiln-hir` analyzes the expression passed to `isolate()`. It proves via escape analysis that:
   * No reference to the root or any child object escapes into the sender's outer scope or global variables.
   * The subgraph contains no references to outer realm-specific intrinsics or globals.
2. **Atomic Unlink and Relink:**
   * During the `transfer` host-op, the runtime takes the contiguous allocation chain for the isolated subgraph.
   * It unlinks the block list from Realm A's `heap->blocks` list ($O(1)$).
   * It splices the block list into Realm B's `heap->blocks` list ($O(1)$).
   * In Realm A, the root local variable slot is overwritten with `undefined` (destructive read).
3. **Performance:** A 500 MB graph transfer completes in **~4 pointer reassignments (< 5 nanoseconds)** with zero allocation copies and zero serialization CPU time.

---

### 4. Pure Data Parallelism (`parallelMap`)

For compute-bound operations over collections, developers use `array.parallelMap(fn)`.

1. **Static Purity Check in `kiln-hir`:**
   During AST lowering, the compiler checks the closure `fn`:
   * Does it mutate any captured variable (`let` binding) from an outer scope? (Refused if yes).
   * Does it call any function with unknown side effects (I/O, DOM, global mutation)? (Refused if yes).
   * If pure, it is tagged `Effect::Pure` in the SSA Value Graph (ADR 0056).
2. **Native C Work-Stealing Loop:**
   `kiln-backend-c` lowers `parallelMap` into a multithreaded loop backed by Kiln’s runtime work-stealing pool (Tokio/rayon):
   * Chunks of the input array are distributed across available worker threads.
   * Threads run the emitted scalar C logic in parallel without entering a full JS realm.
   * The calling realm suspends on a Tokio oneshot receiver and resumes when all chunks complete, returning the resulting native array.

---

### 5. Architectural Invariants

1. **No Shared Mutable Pointers:** A pointer to a `kiln_c99_gc_block` is owned by exactly one `Realm` at any instant. Concurrent access to the same heap block is forbidden by construction.
2. **Zero Cross-Realm GC Tracing:** A Realm's turn-boundary mark-sweep tracer (ADR 0041) only ever scans its own block list. It never traverses pointers into another thread or realm.
3. **Sound Destructive Read:** Once an isolated region is transferred, reading the variable in the sender realm throws a compile-time error (if detected by `kiln-hir`) or evaluates to `undefined`.
4. **Standard Tooling Parity:** Any code utilizing `@kiln/concurrency` must typecheck under standard `tsc --noEmit` and run without modification under Node/Bun via a polyfill using `node:worker_threads`.

---

## Consequences

### Positive
* **Best-in-Class In-Process Parallelism:** Combines the ergonomics of actors, the speed of zero-copy region transfers (~nanosecond handoffs), and multi-core data parallelism without writing locks or atomics.
* **Preserves Kiln’s Core Performance:** Leaves single-threaded execution, property lookups (ADR 0047), and turn-boundary GC (ADR 0041) untouched. Single-core HTTP performance (~111k req/s) is preserved.
* **100% `tsc` Compatible:** Works immediately in existing IDEs, ESLint, and CI pipelines without TypeScript compiler forks.
* **Shared Text Segment:** Since Kiln compiles AOT, worker actors share compiled native code and constants in `.text` / `.rodata` with ~1–2 MB resident memory per actor.

### Negative & Limitations
* **Escape Analysis Conservatism:** `kiln-hir`'s escape analysis will reject complex, ambiguously aliased structures from `isolate()`, requiring developers to structure transferrable data cleanly.
* **Polyfill Overhead in Standard Engines:** When running under Node or Bun via the polyfill, `transfer()` falls back to `worker.postMessage()` (structured clone), losing the zero-copy advantage outside of Kiln.

---

## Alternatives Rejected

1. **Adopting Jarred Sumner’s Shared-Heap PR #249:** Would require abandoning ADR 0041, rewriting the memory manager for concurrent stop-the-world GC, and adding synchronization overhead to every object property read.
2. **Waiting for TC39 Shared Structs:** Still at Stage 2. Forces developers to write manual `Atomics.Mutex` locks, which is hostile to TypeScript's async-first idioms.
3. **Pure Actor Model without Zero-Copy Transfer:** Standard Web Workers with structured cloning are too slow for large data pipelines (50–100x slower than pointer transfer).
