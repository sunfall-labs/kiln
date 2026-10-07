# Synchronous worker wait on bounded realm lanes

Status: architecture investigation, 2026-09-28. This note records a design
boundary; it does not claim an implementation or change defaults.

## The stack that must survive

`Atomics.wait` has a synchronous JavaScript result. The TypeScript builtin calls
the `WAIT` primitive and returns its string. `runtime/arrays.c` calls the
provider's `kiln_atomics_wait`, then returns `"ok"`, `"not-equal"`, or
`"timed-out"` through its caller. A worker's generated
`kiln_cli_worker_advance` enters `kiln_cli_realm_step`; a module body enters
`kiln_realm_enter`, whose Rust executor calls back into generated C. An idle
worker can also enter `kiln_realm_drive_step`. Thus the stack at a wait can
contain emitted C, runtime C, Rust provider and Tokio executor frames.

The existing async/generator frame and `KILN_C99_YIELDED` protocol suspend
only functions the graph marks async or generator. Ordinary synchronous
functions retain locals, SSA values, arguments and exception state in C
frames. The runtime can call JS through dynamic dispatch, array callbacks,
proxy traps and host callbacks. Returning a new `Pending` status from
`kiln_atomics_wait` would unwind those frames and has no path to reconstruct
them. Retrying `Atomics.wait` from the next language turn would re-execute
observable work before the call and is not equivalent to a suspended call.

## Why more lanes and routing do not prove progress

`spawn_pinned` creates the worker session on one owner lane. `Atomics.wait`
occupies that OS thread; `enter_blocking_scheduler` starts a replacement for
movable tickets only. If every base lane contains a waiting parent and every
notifier is a pinned child on an occupied lane, no notifier can run. Routing
each child to a different lane from its parent still permits a cycle (A waits
for a child on B; B waits for a child on A). The
`two_waiting_parents_starve_cross_routed_pinned_notifiers` runtime test proves
the two-lane case using the real waiter list and a bounded timeout. A general
default must make progress with an infinite wait as well as a timeout.

## Candidate: same-lane stackful continuation

The contained direction is to run a scheduled worker's native step on a
separate, guarded stack that stays attached to its owner lane. At a blocking
wait, atomically register a waiter and its scheduler waker, save the whole
execution context, and switch back to that lane's scheduler stack. The lane
can then run its other pinned tickets. `notify`, timeout or termination queues
the original ticket; its owner lane restores the saved context, polls the
waiter outcome under the same waiter-list lock, and returns from the original
`kiln_atomics_wait` call. The waiter registration and ticket transition must
retain a wake that races suspension. Normal end-of-turn returns discard the
temporary stack; an outstanding wait keeps it until resume or cancellation.

This cannot be implemented by wrapping only `runtime/arrays.c` in a C fiber:
`kiln_realm_enter` can have Rust/Tokio frames below the generated callback.
The context switch would preserve those Rust frames too. An implementation
must establish that the compiler, unwinder, Tokio and sanitizers support that
switch on both Linux and macOS, or move the entire realm entry path onto a
validated fiber abstraction. `setjmp`/`longjmp` alone cannot resume a stack
after the scheduler reuses it.

The GC and affinity changes are part of the same feature:

1. `kiln_c99_gc_stack_base` and `kiln_c99_stack_floor_find` currently query
   the pthread stack and cache their bounds in thread-local storage. A fiber
   needs its own high bound and recursion floor. Collection on a running
   fiber must scan its active segment and saved registers. A suspended stack
   must either be scanned as a root of its owning heap or be unreachable to
   that heap's collector until it resumes; that exclusion needs an explicit
   lease assertion, including GC stress and teardown.
2. `ActiveRealm` occupies Rust TLS (`ACTIVE_REALM`, `ACTIVE_HEAP`, worker scope,
   scheduler pool/waker) for the duration of generated code. Runtime C also
   keeps frame arguments, diagnostic depth, new-target state and the stack
   floor in TLS. Switching to another pinned realm on the same lane requires
   saving and clearing the full dynamic context, then restoring it before
   either realm resumes. Merely changing `ACTIVE_REALM` would expose one
   realm's arguments or heap to another.
3. `ActiveRealm` holds a `reclaim::Online` guard. Leaving it online through an
   infinite wait can prevent retired shared cache state from being reclaimed
   indefinitely. Marking the lane quiescent while a suspended C frame holds
   raw cache pointers can free them. The implementation needs a proof that
   no such pointer crosses the suspension, or explicit roots/hazards for
   those pointers before it can release the guard.
4. `ScheduledWorker::run_turn` currently completes a terminated worker
   before calling `advance`. A suspended wait must instead resume on its
   owner lane, receive the interrupted outcome, unwind the generated frames
   and report exactly one `EXIT`. Dropping a suspended stack is not an unwind;
   cancellation, parent teardown and worker exit need an explicit policy for
   native resources held across the wait.
5. Stack reservation and committed pages per sleeping worker must be
   measured against issue 08's memory target. Using the current 64 MiB lane
   stack size for every suspended waiter would defeat bounded thread count
   if many stacks become resident. The recursion guard must still raise a
   catchable `RangeError` before a guard page fault.

## Alternative and gate

A compiler continuation avoids switching Rust stacks by spelling each
possibly suspending call as a heap frame and propagating suspension through
all callers. It must include generated synchronous functions *and* runtime C
operations that call back into JS (for example array methods and proxy
traps), as well as exceptions and `finally`; the current async/generator
frames cover only a subset. This is a compiler/runtime ABI change, not a
single `WAIT` lowering.

Keep the dedicated native worker default and scheduled workers opt-in until
one complete design has focused tests for: nested synchronous waits and
`finally`; two or more waiting parents with cross-routed pinned notifiers;
`notify` before and after park publication; timeout and termination races;
GC stress while another realm runs on the same lane; stack overflow and
teardown; and thread and memory bounds under many sleeping workers. The
two-lane waiter-list test is a failing liveness model for the current
scheduled path, not a proof of a continuation. No smaller routing or ABI
change identified in this investigation satisfies those gates.
