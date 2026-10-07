# Realm turn scheduler: design for issue 09

Status: reviewed and accepted for implementation, 2026-09-27. The owner delegated the
implementation once the architecture review found it sound; this document is not an
implementation claim.

## Ownership and the scheduling boundary

One `Scheduler` owns a fixed set of worker lanes, normally one per available core, and a
process-wide ready queue. A ready ticket names a realm and a generation, never a borrowed
realm pointer. An atomic state transition (`idle -> queued -> running -> idle`) permits
exactly one lane to run a realm. A stale ticket whose generation differs is discarded.
Each lane can steal work from the others. A realm runs exactly one language turn before
rejoining the queue; the lane releases all generated frames and all per-thread borrowed
references before it publishes `idle`. Reclamation's `quiescent` call and a collector
opportunity occur at this boundary, before another lane can acquire the realm.

The native realm and its heap live in a stable allocation owned by a scheduler realm
record. The record has an exclusive running lease; moving that lease between lanes is
the only way to move a realm. `ACTIVE_REALM`, the heap cache, and emitted per-thread
frame registers are installed on entry and cleared on exit. The provider's current
thread-local `REALMS` registry becomes a process registry of handles to records; a
lookup only returns the current lane's leased realm to generated code. This is the
safety argument for making the realm state movable despite its interior `Cell` and
`RefCell` use: no other thread may touch those fields while a lane holds the lease.
Pointers retained *inside the realm* (for timers, continuations and GC roots) can
move with that realm; pointers and `Rc` clones visible to another thread cannot.
The implementation needs an explicit audit of every such reference. A deliberately
small `Send` envelope may use an unsafe implementation only after that audit; making
`NativeRealm` itself unconditionally `Send` would hide the pinning condition.

The generated launcher must split initialization, one-turn drive, and teardown into
separate entry points. `kiln_cli_run_realm` currently keeps the whole program on a C
stack and calls a single `kiln_realm_drive`; scheduling the whole function is still
one thread per realm. The initializer returns only after generated frames have
unwound; the scheduler owns the realm record until the final turn and runs teardown
on a lane with the realm entered. Each one-turn call returns `Ready`, `Parked` (with
a wake/deadline), or `Done(status)`. Tokio readiness or a timer posts one ticket per
realm generation. A wake racing the `running -> idle` transition is retained in the
record, so no notification is lost.

## Executor and pinning

The scheduler has one process-wide Tokio I/O runtime, not one runtime per realm.
The current `Executor` bundles a current-thread runtime with a `LocalSet`; that
bundle must be split. The reactor runs at most one dedicated Tokio worker thread;
each scheduler lane enters its handle and polls only the `LocalSet` belonging to
the realm it leased. A realm with live local tasks keeps that task set on its lane
and is pinned until those tasks finish. An idle movable realm carries no `LocalSet`
across threads; its next lane creates an empty task set on demand. HTTP connections
currently pin through their local tasks. The virtual clock is realm state and is
advanced by turn dispatch; Tokio's paused clock is executor state and is never
carried with a migrating realm. Deterministic mode has its own single paused lane
and runtime; wall-clock realms never run on that paused reactor. Pure timers are
scheduler deadlines, not a reason to allocate a Tokio executor for every idle realm.

Loading a native addon, registering a foreign callback, or entering `Atomics.wait`
pins the realm to its current lane for its lifetime. The pin decision is made before
the operation can publish a thread-affine pointer. Existing FFI and HTTP callbacks
therefore stay on the same executor. Each lane has a local ready queue and a local
executor for its pinned realms. A pinned realm that parks for I/O or a timer releases
the lane and its wake queues a ticket **on that lane**; many parked pinned realms
share one lane and one executor. The reactor does not create an OS thread per realm.
A synchronous blocking operation keeps its current OS thread; the scheduler starts a
replacement worker for movable tickets while that thread is blocked and retires it
after the blocker resumes. A replacement cannot run another realm pinned to the
blocked thread: its executor and foreign pointers are thread-affine. Such work waits
for the original thread, so the pinning audit must identify synchronous blockers and
the acceptance probes must exercise a blocker alongside other pinned realms on the
same lane. Where Node semantics permit it, `Atomics.wait` should park a continuation
and release the lane; otherwise it follows the blocking rule, with the delay to pinned
siblings measured and documented. The normal idle pool remains cores plus the reactor.
Replacement workers are bounded by the number of blocked turns, with a configurable
hard limit that reports exhaustion instead of silently starving movable work.

## One pool for realm turns and parallel chunks

The same scheduler queue carries `RealmTurn` and `ParallelChunk` tickets. The
provider's private Rayon pool is removed, including its separate worker budget.
`kiln_parallel_map` keeps its measured prefix and chunk-size probe, then submits
the remaining chunks to the scheduler. Its calling lane executes and steals chunks
while joining, so all lanes may invoke a parallel built-in at once without deadlock.
The join completes before the borrowed input/output/context pointers return to C.
Each chunk ticket names its source realm and enters that heap's no-write mode before
reading it. It may write only its private result arena; the source realm's mutator
stays parked in the join until every chunk has left no-write mode and reported a
quiescent point. A lane running a chunk temporarily clears any prior realm context
and restores it before returning to its own queued realm turn. The frozen-heap proof
from ADR 0069 still decides whether a callback may become a chunk; scheduler capacity
never turns an effectful callback into a parallel one.
Parallel sort uses the same queue and an explicit stable merge plan, preserving its
current tie and `NaN` behavior. `KILN_PARALLELISM=1` selects one scheduler lane, and
the deterministic mode selects one lane and FIFO ticket order with virtual time.

## Migration and GC safety

The collector's mid-turn conservative scan must capture the current lane's stack
base on each entry, not cache the first thread's stack base in the heap. At a turn
boundary, all generated frames have returned and explicit realm roots suffice for
collection. Every pointer kept by a callback, completion, timer, or emitted unit
must either be rooted in the realm record or trigger pinning. The scheduler's
exclusive lease protects the heap and per-realm registries; it does not protect
process-wide keys, shapes, or cache slots, whose release/acquire publication and
turn-based reclamation remain as ADR 0071 specifies.

## Staged implementation and evidence

1. Extract a synchronous, one-turn native drive and split the generated launcher.
   Keep every realm pinned and confirm the native corpus and HTTP close contract.
2. Introduce scheduler tickets, wake coalescing, deadlines and deterministic FIFO
   mode, still with every realm pinned to its lane. Verify the 05 harness at
   1, 2 and core-count lanes, including repeated timer and message probes.
3. Move the provider registry and GC stack-base rule behind the exclusive lease.
   Audit every retained `Rc`, raw pointer and thread-local path, and implement
   pin transitions before addons, FFI callbacks, `Atomics.wait`, and executor-local
   HTTP work can publish thread-affine state. Until that audit passes, tickets
   never migrate. Run the affected native corpus with GC stress and ThreadSanitizer
   on the realm harness.
4. Allow only audited, unpinned realms to migrate at turn boundaries. A wake may
   queue on another lane only after every generated frame has returned and the
   old lane has cleared its realm context. Repeat the 05 and deterministic-mode
   harnesses with lane identity recorded and assert that eligible realms actually
   move while pinned realms never do.
5. Put parallel chunks and stable sort onto the shared queue; remove the private
   Rayon pool and its budget. Compare all parallel probes at 1, 2 and maximum lanes
   and measure the sequential overhead.
6. Add blocking replacement lanes. Exercise FFI, addon, `Atomics.wait` and HTTP
   affinity with explicit worker identity assertions.
7. Run 10,000 idle realms in one process and the one-realm-per-connection benchmark.
   Record resident memory, worker thread count, wake latency and throughput against
   Node and the current pinned implementation. A quiescent run must have no more
   than cores plus one reactor thread.

Each stage is a separate reviewable commit. The final `mise run gate`, native
stress and native-realms runs follow the completed integration, not each stage.

## Synchronous worker waits and the remaining scheduling limit

The scheduled worker bridge remains opt-in (`KILN_WORKER_SCHEDULER=1`). In the
single-lane deterministic scheduler it falls back to a dedicated worker. The
native `Atomics.wait` path explains why a general default needs more work:

- `kiln-builtins/src/atomics.ts` implements synchronous `wait` by calling the
  `WAIT` primitive and returning its string result in the same JS call.
- `kiln-backend-c/src/runtime/arrays.c` calls `kiln_atomics_wait` from the
  generated call stack, then converts its integer result to `"ok"`,
  `"not-equal"`, or `"timed-out"` before returning to that stack.
- The provider's `kiln_atomics_wait` pins the realm, starts a bounded
  replacement lane, and blocks in the runtime's mutex/condition-variable
  waiter list. `notify` wakes that OS thread. The synchronous C ABI has no
  `Pending` result or continuation handle.

A replacement can run movable tickets, but it cannot run a pinned child on
the blocked lane. Routing each child to a different lane from its parent is
also insufficient: with two lanes, two parents can block in `Atomics.wait`
while each child's notifier is pinned to the other occupied lane. The bounded
`two_waiting_parents_starve_cross_routed_pinned_notifiers` runtime test
exercises the real waiter list and shows that a replacement processes movable
work while the pinned notifiers wait for a parent's timeout. A zero-timeout or
already-mismatched wait does not have this problem.

Returning `Pending` from the provider cannot solve the blocking case: the C
and generated JS frames above the call would unwind, losing the place where
the synchronous result must resume. `setjmp`/`longjmp` cannot resume a stack
after the lane has reused it. A stackful fiber could preserve those frames,
but would need explicit GC stack scanning, TLS/affinity handling, portable
context switching and a stack budget per blocked worker. A compiler-driven
continuation would make the suspension explicit, but must transform every
potential caller of `Atomics.wait`, including nested calls and exception
paths. Neither is a local ABI change. Until one is implemented and tested,
scheduled workers cannot safely become the default for programs that may
block on another scheduled worker; dedicated worker threads remain the safe
fallback.

## Actor scheduling decision, 2026-09-28

The actor extension has a narrower contract than `node:worker_threads`: its
mailbox calls are asynchronous, and an actor refuses synchronous
`Atomics.wait`. Actor realms therefore enter the bounded scheduler by default,
using a process-shared pool for the parent's clock when the parent is an ordinary
single-realm launcher. Virtual-clock actors use one deterministic lane;
wall-clock actors use the bounded shared reactor pool. Node-compatible workers
remain dedicated by default, preserving
their synchronous wait semantics. The actor restriction is checked both in the
worker module and in the native wait provider so a saved method reference
cannot block a scheduler lane. Native `waitAsync` now registers on the same FIFO
waiter list as `wait` and parks an async host request; `notify` wakes it directly.
