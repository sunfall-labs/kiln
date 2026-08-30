# Bounded `gc-arena` feasibility spike

Checked 30 August 2026. This report is deliberately scoped to the proposed collector gate in
ADR 0029. No production source or workspace manifest was changed.

## Result

`gc-arena` is a plausible implementation for a Rust-owned Realm Heap, but the repository is not
ready to switch its `Value` representation. The collector fits the most important ownership
property: one JavaScript realm is single-threaded, and `gc-arena` is single-threaded with exact
tracing, generative `'gc` lifetimes, and mutation xor collection. Its no-compaction behaviour is
also compatible with stable identity, although it does **not** make raw pointers safe to expose.

The standalone probe in this directory compiles and runs:

```text
cargo run --manifest-path .scratch/realm-heap/Cargo.toml
Finished `dev` profile
Running .../kiln-realm-heap-spike
```

It derives an exact tracer for a two-node cycle, proves that an unreachable node is swept while a
cycle remains live, and exercises `DynamicRootSet` for a continuation-style root. This establishes
library mechanics only; it does not claim interpreter/native readiness.

Recommendation: do a Rust-only `RealmHeap` tracer bullet after the Tokio Realm Runtime has
established Realm ownership and continuation checkpoints. The heap spike may proceed in parallel,
but production integration should wait for the runtime seam. Tokio must precede any native heap
integration because it supplies the one Realm owner, task affinity, teardown point, and completion
queue where all roots can be enumerated. A collector cannot make the current per-translation-unit
C statics into Realm state.

The crate documents the relevant constraints directly: exact `Collect` tracing, generative `Gc`
lifetimes, mutation xor collection, single-threaded arenas, no moving/compaction, and extra effort
for generators/async ([gc-arena README](https://github.com/kyren/gc-arena#readme)). Its
`DynamicRoot` API intentionally uses an unsafe lifetime conversion internally and marks handles
`!Send`/`!Sync`; this is suitable only behind a Rust Realm owner, never as a public C value
([DynamicRoot docs](https://docs.rs/gc-arena/latest/gc_arena/dynamic_roots/struct.DynamicRoot.html)).

## Current interpreter ownership graph

The current `Value` is an enum whose heap variants each carry `Rc`; mutable variants add
`RefCell`/`Cell`. The relevant definitions are in
`crates/kiln-interpreter/src/lib.rs`:

| Current node/edge | Evidence | Collector implication |
| --- | --- | --- |
| Ordinary object | `ObjectData` owns properties, `prototype`, and boxed `primitive` (lines 1054–1083) through `Value::Object` (around 2430) | Exact trace of every property/accessor value and prototype edge; `RefLock`-style mutation is required instead of `RefCell`. |
| Arrays | `ArrayData` has `items: RefCell<Vec<Value>>` and named properties | Dense payload and named properties must be traced as separate fields; an array can point to itself. |
| Functions/closures | `Closure` owns upvalue cells, captured `this`/arguments/`new.target`, prototype and static properties (lines 2259–2300) | The prototype's `constructor` points back to the closure; captured cells can form arbitrary mutually recursive cycles. |
| Cells | `type Cell = Rc<RefCell<Value>>` | A cell is a heap edge, not a scalar slot. A closure/cell/object cycle is not handled by refcounts. |
| Generators/async | `GeneratorState` owns closure, saved locals, operand stack, arguments and handlers (lines 2204–2238) | Suspended state is a root-bearing continuation. It must be traced while parked and while resumed; this is the crate's documented hard case. |
| Promises | `PromiseState` owns settlement and `Reaction`s; `Sink` can point to promises, combinators, generators and settled values (lines 3530–3610) | Pending reactions and adoption jobs are graph edges. The microtask queue is also a root set. |
| Promise combinators | `Combinator` owns a `Vec<Value>` and result promise | Must trace both values and the result; reactions can keep the combinator alive after user references disappear. |
| Proxies/bound functions | `ProxyData` owns target + handler; `BoundFunction` owns target, receiver, arguments | Direct edges may be acyclic alone but can close cycles through object properties or closures. |
| Views/buffers | TypedArray/DataView own `Rc<BufferData>` (lines 1950–2134) | The backing bytes are payload; views are edges to one buffer and must not outlive it. |
| Regex/date/BigInt/symbol | `Rc` around mutable/date, regex engine state, BigInt, or identity state | These are leaf or external-resource nodes only if their ownership/finalizer policy is explicit. They cannot be casually put in a `Gc` type with normal `Drop`. |

The VM itself is the root set. Its fields include `stack`, `locals`, `arguments`, `this_value`,
`new_target`, `frames`, `handlers`, `global_object`, `process_object`, `error_prototypes`,
`timers`, `microtasks`, `caught`, `unhandled`, `symbol_registry`, and suspended-generator state
(around lines 3633–3753). Every one can contain a `Value`, directly or transitively. A frame also
holds `program` and `function`; `Program` is immutable compiler-owned data and should remain
outside the Realm Heap, while function identity and all captured values belong inside it.

The current manual reclamation is intentionally incomplete: `Vm::release_cycles` (lines 3818–3840)
walks a weak closure registry, clears closure prototype/properties/upvalues, and empties selected
realm roots. It does not perform graph tracing for self-referential arrays/objects, promises,
proxies, generators, or arbitrary cycles. It is sound only when no returned value remains, which is
why it is used by `execute_reclaiming`, not ordinary `execute`. A collector can delete the weak
closure registry and this destructive cycle-cutting pass; semantic path guards used by inspection,
serialization, and prototype traversal must remain.

## Current native C ownership and ABI

The generated C runtime is a separate heap implementation. Internal `kiln_value` extends the public
five-scalar ABI with pointer-bearing tags (function, cell, object, array, generator, collection,
promise, settler, bound, symbol, regexp, BigInt) in `crates/kiln-backend-c/src/lib.rs` around
lines 190–270. The public embedding value remains a 32-byte `{tag, reserved, bits, ptr, len}`
record in `kiln-native-runtime` and only carries scalar tags (lines 333–360).

The C side allocates graph nodes with raw `malloc`/`realloc`: objects and arrays (lines 1135–1162),
closures/upvalues (around 2745–2790), suspended generator locals/stack/handlers (around
1388–1438), promises/reactions, settler and bound argument arrays (around 9534–9657), collections,
symbols, regex/date state, and timer arguments. Most graph allocations have no matching teardown;
the `free` calls visible in the file are predominantly temporary text/UTF-16 buffers and failed
allocation paths. C values in `bits` are raw addresses, so they have no arena brand, no owner, and
no safe way to tell whether the pointer belongs to one Realm or another.

The native scheduler and Realm state are also file-local statics: `kiln_c99_globals`, timer table,
virtual clock, timer IDs/sequence, microtask queue, random state, and temporary frame argument
state (around lines 909–914, 2119–2170, and 9495–9505). The C event loop drains those queues
around lines 9668–9713. That is precisely the state ADR 0028 moves under one Tokio-owned Realm;
`gc-arena` alone cannot fix cross-module ordering or teardown.

Ordinary generated C calls use automatic `stack`/`locals` arrays and C recursion. The only heap
frame is `kiln_generator`, whose saved arrays are separately allocated; the C runtime explicitly
limits recursion to 256 rather than registering a collector frame chain. ADR 0021's frame-chain
and safepoint prerequisite is therefore not met. A precise collector needs every generated frame,
the current operand/local arrays, handlers, and C-to-Rust re-entry arguments to be visible at a
Realm-safe checkpoint.

The private Rust provider ABI currently exposes scalar/regex/BigInt functions and opaque provider
handles, but no Realm or heap ownership handle. A future shared heap must preserve ADR 0028's rule:
no Rust layout, `Gc`, future, waker, or Tokio type crosses C. Generated C must use a private,
versioned API with an opaque Realm/Value/continuation handle and explicit disposal/teardown. A
`DynamicRoot` itself is unsuitable as a C token: its fetch path relies on a same-arena lifetime
brand and its documented unsafe conversion, and the type is `!Send`/`!Sync`.

## What the collector could delete

After a complete Realm migration, `gc-arena` can remove or consolidate:

* `Rc`, `Weak`, and `RefCell` ownership from Realm Heap nodes (`Object`, `Array`, `Closure`,
  `Cell`, `GeneratorState`, promises, combinators, proxy/bound values, and view/buffer graph
  edges). `RefLock`/`GcLock` or mutation-context APIs replace direct interior mutation.
* `Vm::closures` and `release_cycles`; collection becomes reachability-based rather than a weak
  registry plus destructive edge cutting.
* The native C `malloc` object graph, if and only if all graph operations are routed through the
  Rust-owned runtime and the C side stores opaque handles rather than raw `Gc` addresses.
* Duplicate object/promise/generator graph allocation in the C implementation. The Tokio Realm
  migration separately deletes the generated C timer/microtask loop and file-local scheduler
  state.

It does **not** delete: immutable `Program`/IR data, compiler allocations, temporary conversion
buffers, HostOps-owned resources, or semantic cycle guards for `JSON.stringify`, inspection, and
prototype traversal. Regex and BigInt handles need a separate explicit owner/finalizer policy;
putting a `Drop`-bearing Rust engine inside a collected node would violate the collector's safety
contract unless deliberately isolated.

## Blockers before production adoption

1. **Exact root inventory.** The full VM root set above must be represented by one `HeapRoots`
   value, including parked generators, handlers, timers, microtasks, pending promise reactions,
   host-caught values, realm globals, exports/imports, and temporary values held across a native
   call. Missing one edge is a use-after-free, not merely a leak.
2. **Representation/API rewrite.** `Value` currently clones `Rc` cheaply and borrows `RefCell`
   directly throughout a 17k-line interpreter. `Gc` values are branded and cannot escape arena
   callbacks; mutating methods must receive a `Mutation` context and use barrier-aware locks. The
   current function signatures and helper methods are not collector-neutral.
3. **Mutation xor collection.** Collection can run only when no `Gc`-using code is executing. The
   VM needs explicit safe checkpoints between instructions or turns, and all Rust re-entry paths
   (`map`, sort, promise reactions, generators, accessors) must return to that checkpoint before
   collection. A long `run_frames` call cannot collect in its middle without a generated safepoint
   protocol.
4. **Generators/async continuations.** `GeneratorState` is currently `Rc<RefCell<_>>` and can be
   held by a pending promise reaction. The suspended state must be a traced node or an explicit
   dynamic root, and resume/drop ordering must be tested through rejection, `.return()`, and
   cancellation paths.
5. **Native frames and opaque handles.** C automatic locals/stack/handlers are not traced. The
   native runtime must first expose frame registration/safepoint polling and convert C values to
   Realm-owned opaque IDs. It must reject stale/wrong-Realm handles and prohibit callbacks after
   Realm teardown.
6. **Resource ownership/finalization.** `regress::Regex`, BigInt provider handles, byte buffers,
   and host resources cannot be assumed to have a safe GC finalizer. Keep them in explicit Realm
   registries or define a finalizer contract and test teardown on every error/cancellation path.
7. **Cross-module identity/lifetime.** Exported functions and mutable export cells live beyond one
   module VM. A per-run arena would invalidate them; the arena must be Realm-scoped and outlive the
   complete module graph and all queued work.
8. **Cost evidence.** Measure allocation throughput, peak bytes, full-cycle pause, incremental
   debt, and differential execution against the current interpreter. `gc-arena` is incremental,
   but the current interpreter has no allocation/collection budget to compare against.

## Minimum safe tracer bullet

The smallest production-shaped experiment that can earn the collector decision is not a generic
linked-list demo. It should be a Rust-only `RealmHeap` module, with no generated-C integration yet,
that implements a deliberately narrow value subset but the complete root/continuation protocol:

1. `HeapValue` covers scalar values plus object properties/prototype, arrays, cells, and closures
   (including the closure↔prototype constructor cycle). Keep `Program` and HostOps external.
2. `HeapRoots` contains VM stack/locals/frames, parked generator state, timers, microtasks,
   caught/unhandled values, and Realm globals. All fields are traced by derive or one audited
   `Collect` implementation.
3. A single `Realm::mutate` operation executes a small callback, then returns to `Realm::checkpoint`
   where `collect_debt`/`finish_cycle` may run. No collection occurs inside a value operation.
4. Add a private continuation-root table backed by `DynamicRootSet`. The table is Realm-owned,
   local-task-only, has explicit register/complete/cancel operations, and has no public `Gc` or
   lifetime-bearing type.
5. Keep the existing interpreter as the oracle. Run the same programs through the old `Rc` VM and
   the experimental heap for allocation, property mutation, closure capture, suspension, and
   teardown. Do not switch the main `Value` type until all tests below pass.

The native follow-up is a separate tracer bullet: one generated C function can create an object,
store a closure in it, suspend on one private continuation, complete it through the Rust Realm, and
then tear down. The C shim sees only opaque `uint64_t`-style handles and must never reconstruct a
`Gc` pointer. This should happen only after ADR 0028's Realm Runtime owns the call and completion
path.

## Exact success/failure tests

The spike passes only if every test is deterministic and both positive reachability and negative
collection are demonstrated. Any missing tracer edge or unsafe C handle is a failure, not a waived
case.

### Rust Realm Heap tests

* **Cycle collection:** make an object self-cycle, a two-object prototype/property cycle, an array
  containing itself, and mutually recursive closures through cells. Drop all external roots;
  `finish_cycle` must mark them dead (using `GcWeak`/finalization instrumentation) without panic.
* **Diamond retention:** two live objects share one child; collect after dropping one parent and
  prove the child survives, then collect after dropping the second and prove it dies.
* **All VM roots:** put one object only in each root slot (stack, local, frame, handler payload,
  current `this`/`new.target`, timer callback/args, microtask callback/arg, caught, unhandled,
  global/process/error prototype, and symbol registry). Each must survive collection; remove it and
  prove collection.
* **Promise graph:** pending promise → reaction → derived promise; adoption; combinator slots;
  rejected reaction; and async `Sink` generator/promise. Verify settled values and callbacks remain
  alive until the queue drains, then become collectible.
* **Suspension:** suspend a generator at `yield` and an async body at `await`; collect at every
  allowed checkpoint; resume successfully; then close/cancel and collect the continuation.
* **Mutation barrier:** mutate an old object to point to a newly allocated object during an
  incremental cycle; the new child must survive. Exercise the same write through every mutable
  container kind.
* **Arena isolation:** a handle from Realm A cannot fetch in Realm B; wrong-Realm and post-teardown
  handles return a deterministic error and never dereference memory.
* **No Rust lifetime leakage:** compile-fail tests must reject returning a `Gc` from a mutation
  callback, storing it in a `'static`/TLS location, or moving it across threads.
* **Resource policy:** regex/BigInt-like external resources are either explicitly Realm-owned and
  disposed at teardown or are proven not to be stored in collected nodes. No finalizer calls into
  a dead Realm.
* **Oracle differential:** for a bounded corpus, old `Rc` execution and the heap experiment agree
  on result, errors, promise/timer order, and observable identity.

### Native private-ABI tests (after Tokio Realm Runtime)

* **Root registration:** C registers each live handle before a call/checkpoint and unregisters it
  on completion; collection never invalidates a registered value.
* **Stale/wrong Realm:** forged, double-completed, post-cancel, post-teardown, and cross-Realm
  handles fail with the documented status and do not call C function pointers.
* **Frame safepoint:** a generated function with object locals/stack polls at entry and at the
  agreed checkpoint; all values survive while the frame is registered, and disappear after return.
* **Suspended continuation:** C suspends, Tokio wakes/completes the request, Rust queues the
  continuation on the owning Realm, and C resumes only on that Realm task.
* **Graph-wide queue:** two native modules share one Realm; enqueue jobs/timers in module A and B
  and assert one FIFO/microtask + `(deadline, sequence)` timer order.
* **ABI audit:** headers expose only scalar public values and opaque private handles; no Rust
  layout, `Gc`, Tokio, future, waker, allocator, or function-pointer lifetime crosses the stable
  embedding seam.
* **Teardown stress:** repeatedly create/drop a Realm with pending timers, promises, generators,
  and continuations under success, host error, thrown JS error, cancellation, and allocation
  failure policy. Sanitizers must report no leaks/use-after-free in the supported ownership model.

### Performance gate

Record at least: allocations/sec for scalar/object/closure/promise-heavy programs, bytes per live
node, peak heap for a long promise chain, full-cycle pause, incremental collection work per
checkpoint, and native handle-call overhead. Compare against the current `Rc` interpreter and the
existing C implementation. A collector that is correct but makes ordinary interpreter execution or
native calls materially worse fails the gate until the cost is understood.

## Ordering decision

Tokio Realm Runtime should precede shared heap production work, while the standalone Rust spike can
and should run now. The reason is ownership, not a collector preference:

* ADR 0028 gives each Realm one local task, explicit completion queue, teardown, and async
  continuation owner. Those are exactly the places the collector needs to know when mutation ends,
  when roots are parked, and when it is safe to collect.
* The current C implementation has per-unit static globals and no frame/safepoint chain. Migrating
  it to `gc-arena` first would either create a second Rust/C heap or smuggle raw pointers through
  the private ABI, violating ADRs 0021, 0028, and 0029.
* Once the runtime seam exists, the collector can be inserted behind it: interpreter and native
  adapters share `RealmHeap`, and the C side uses opaque handles plus explicit root registration.

Therefore: approve `gc-arena` only as a conditional candidate; land the Rust-only tracer bullet
and its exact tests; complete the Tokio Realm Runtime ownership slice; then attempt the native
opaque-handle/frame-safepoint slice. Any failure in exact roots, suspended continuations, C handle
safety, finalization, or the performance gate leaves ADR 0021's collector choice open.
