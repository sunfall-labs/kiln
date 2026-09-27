# ADR 0040: The executor is not the realm state

## Status

Accepted. Implemented (2026-09-26; see *What step 4 did*, below). This recorded the shape before
the work started, in the same spirit as ADR 0016 — the choice is cheap to make now and expensive
to reverse once the split has been half-made in one direction.

## Context

`RealmRuntime` owns two unrelated things in one struct:

```rust
pub struct RealmRuntime<H = TokioHost> {
    runtime: tokio::runtime::Runtime,          // the executor
    local: tokio::task::LocalSet,              // the executor
    host: H,                                   // realm state
    capabilities: RealmCapabilities,           // realm state
    http_connections: BTreeMap<ResourceId, RuntimeHttpConnection>,  // realm state
    http_agent: HttpAgent,                     // realm state
    runtime_metrics: tokio_metrics::RuntimeIntervals,
    time_paused: bool,
}
```

All 41 of its methods take `&mut self`; 16 of them touch the executor fields.

That bundling has one consequence, and the consequence is the whole reason this ADR exists: **the
native drive cannot hold the executor across a drive, because the drive body needs the state.**
`block_on` would have to borrow the struct for the lifetime of the future, and the future mutates
`host` and `http_connections` on every turn. The only way out within the current layout is to
re-enter the executor for each small step — which is exactly what the native adapter does.

### What that costs, measured

Counting runtime entries under a 16-connection load, over 14,175 requests:

```
poll_http_tasks_once (block_on)   ~120,000   ~8.5 per request
next_turn                            26,673   ~1.9 per request
parks on the blocking select!            13   essentially zero
```

Two readings matter. The server **never waits on the network** — the blocking wait fires thirteen
times in an entire run — so the 55% of samples in `kevent` (against 32 in `recvfrom`/`sendto`
combined) is not idle time. It is park overhead from `block_on` entries that had nothing to wait
for. And `yield_now().await` is the difference: inside `block_on` it must park, because the future
returns `Pending` and the driver goes to the kernel for a self-wake; inside a task on the same
`LocalSet` it is a userspace reschedule.

Three tuning passes have already taken the 16-connection case from 141 to 4,973 rps by reducing how
*often* the runtime is re-entered — a redundant per-connection scheduler drive, a per-microtask
checkpoint, and a fixed drain replaced by one that stops at quiescence. Those were real, and they
are finished: what remains is the per-entry cost, which no amount of frequency reduction reaches.

### The duplication this already caused

The adapter carries 21 `in_context` references and 8 `if in_context {}` forks; the runtime exposes
11 `_in_context` method variants — `poll_http_connection` beside `poll_http_connection_in_context`,
`start_host_op` beside `start_host_op_in_context`, and so on. Every pair answers one question: *am I
inside the executor or outside it?* The `_in_context` half is the one that must not nest `block_on`.

They are not an accident either. They are what the bundled layout forces once generated C — which is
synchronous and re-enters host operations from inside a callback — has to run somewhere.

## Decision

**The executor and the realm state become separate owners.**

```rust
struct Executor { runtime: tokio::runtime::Runtime, local: tokio::task::LocalSet }
struct RealmState<H> { host: H, capabilities: ..., http_connections: ..., http_agent: ... }

executor.block_on(drive(&mut state, host, thrown))   // one entry per program run
```

The drive holds `&mut RealmState` for its lifetime while the executor is held separately, so there is
no aliasing to launder and no nesting to avoid. `yield_now().await` inside the drive becomes what it
should always have been: a reschedule that lets Hyper's tasks run, costing no syscall.

**Everything becomes in-context, so the `_in_context` family is deleted rather than extended.** This
is the test that the split is the right decomposition: the faster version is also the smaller one. A
change that made this faster while keeping both halves would be a tuning pass wearing an
architecture's clothes.

**The interpreted path is the target shape, not a new invention.** It already does exactly this:

```rust
self.local.block_on(&self.runtime, drive_realm(realm, &mut execute, &mut checkpoint))
```

One entry for the whole program, an `async fn` looping inside it. The native path diverged only
because it is entered from C. After this ADR the two paths differ in what they execute, not in how
they are driven, which is what ADR 0028 says a single runtime is for.

**Determinism is unaffected, and that is a constraint rather than a hope.** The runtime stays
current-thread, the realm stays single-threaded, and the drive still decides callback order
explicitly. ADR 0015's virtual clock, ADR 0012's virtual-time event loop and ADR 0026's seeded
generator all key on the realm's own sequencing, not on how the executor is entered. `scripts/native.sh`
holding at 0 diverged is the standing check that this remains true.

**The C ABI does not change.** `kiln_realm_drive` still takes the same arguments and returns an
`int`. Only what is behind it moves.

## Alternatives rejected

**Make the methods take `&self` and put the state behind an inner `RefCell`.** This works — tokio's
`block_on` takes `&self` — and it is a smaller diff. It is rejected because it keeps the two things
in one type and buys the borrow checker's silence with a runtime check: a drive that re-entered
would then panic on an already-borrowed `RefCell` instead of failing to compile. The bundling is the
defect; hiding it behind interior mutability preserves it.

**Multi-threaded runtime.** Node is single-threaded and reaches 88,000 rps on the same machine, so
threads are not what is missing. It would also cost the determinism above. Rejected on both counts.

**Keep tuning the entry count.** Already done, three times, and the counters say what is left: ~10
entries per request, of which 8.5 are one call site. Removing entries one at a time cannot reach a
per-entry cost.

## Consequences

- The `_in_context` methods and the `if in_context {}` forks are deleted. Net less code.
- Every `block_on` reachable from a JS callback must be gone before the drive is flipped, because
  tokio panics on nesting. That is the migration's real work, and it fails loudly rather than
  silently — a nested entry panics, it does not corrupt.
- The unsafe `as_ptr()` escapes in the native adapter, which exist to hand the executor to a callback
  while the realm is borrowed, lose their reason to exist.
- Expected throughput is not stated here on purpose. The target is ~112 µs of a 204 µs request and
  the mechanism is confirmed, but the honest position is that removing the dominant cost reveals the
  next one. The number goes in the commit that earns it.

## Sequence

1. Split the types, mechanically, with no behaviour change. `scripts/native.sh` at 0 diverged after.
   *Done.*
2. Split the operations to match the types, so the fence is structural. *Done.*
3. Stop the checkpoint from parking. *Done.*
4. Flip the drive to `async fn` and delete the `_in_context` family. *Done.*

Step 2 moved the nineteen executor-free operations onto `impl RealmState` and left the
twenty-two that park on `impl RealmRuntime`. Membership was not chosen by hand: an operation
stays on the runtime if it calls `block_on`, touches `self.executor`, or transitively calls one
that does. That closure is what put `start_http1_connection_with_options` and
`start_http1_client_connection` on the runtime side — they spawn onto the `LocalSet` — and what
pulled `poll_http_tasks_after_wake` and `next_referenced_turn` across with their callees.

`RealmRuntime` dereferences to its state, so no call site changed: a realm is its state plus the
executor that drives it, and code reaching for state reads through the runtime as before. The
conversion is deliberately one-way. State cannot reach back to the executor, which is the whole
point — the async drive of step 3 will hold `&mut RealmState`, so a nested park stops being a
runtime deadlock and becomes a name that does not resolve.

The type system carries that argument but does not finish it, because `Handle::current` reaches
the ambient runtime without going through `self`. `realm_state_operations_never_park_the_thread`
closes the gap by reading the crate's own source and failing if `block_on`, `Handle::current`, or
`self.executor` appears inside the state block. It was confirmed to fail on an injected
violation; a guard that cannot fail is not a guard.

Each step is independently verifiable, which matters more here than usual: this is the component
where a subtle aliasing mistake would be both easy to make and hard to see.

## What step 3 measured

The drive was already down to one `block_on` per call; the parks that mattered were the ones the
checkpoint made from inside it. `poll_http_tasks_once` was a `block_on` around a `yield_now`,
called several times per request, and arming the I/O driver was the whole cost of it: 14,175
requests parked for real 13 times. It now polls the `LocalSet` directly and returns.

A ten-line HTTP server, `bench/server/drive.mjs`, 5s at concurrency 16, three interleaved rounds
per side on one machine:

| | rps | p50 | p99 |
| --- | --- | --- | --- |
| before | 4,603 / 4,713 / 4,761 | 3.46 ms | 4.59 ms |
| after | 8,875 / 9,155 / 9,138 | 1.73 ms | 2.55 ms |

1.93x throughput, latency halved, no errors on either side. Node on the same box serves 88,424
rps, so this closes about a twentieth of the gap rather than the gap; the next cost is whatever
the profile says it is once the parks are gone, which is the honest position stated above.

The correctness argument is that the checkpoint's termination condition did not change. A round
that applies no actions and drains no events still ends it, and readiness that needs the kernel is
still discovered where it always was: the drive's own await parks, and the work resumes through
`poll_http_tasks_after_wake`. Confirmed by conformance at 45/45 and `scripts/native.sh` at 53
matched, 0 diverged - the gate that a previous, unsound attempt at this hung for ten minutes.

## What step 4 did

Most of the flip landed with the bridge's deletion (f9ea9574): the native drive became one
`block_on` around an `async fn`, and the eleven `_in_context` twins went. What that left was the
question they answered, asked in one place instead of eleven — `Executor::run` checked
`Handle::try_current()` and either ran the operation or wrapped it in a `block_on` — and the
adapter still reached the realm through a `RefCell<RealmRuntime>` plus a raw `*mut RealmRuntime`
thread-local, taken with `as_ptr()`, so that callbacks could get at the executor while the drive
had the realm. Step 4 finished both.

- `Executor` is a public type that owns the Tokio runtime, the `LocalSet`, the paused-clock flag
  and the metric stream; all four are properties of a runtime, not of a realm. It has one parking
  entry (`block_on`) and one non-parking one (`scope`, which makes the executor ambient through
  `Runtime::enter` and `LocalSet::enter`). Both guards nest, so nothing asks "am I in context".
- `RealmState` is built by `RealmState::new(host, capabilities)` with no executor, and every
  operation that spawns or registers I/O — `start_host_op`, the HTTP attaches, `http_close` —
  lives on it and expects to run inside a drive. `RealmRuntime` is now `Rc<Executor>` plus a
  `RealmState`, with a wrapper of the same name for each such operation that enters `scope`
  first; the interpreter, daemon, conformance and worker pool call it unchanged.
- The native adapter holds `Rc<Executor>` and `RefCell<RealmState>` side by side.
  `kiln_realm_drive` enters the executor once and the drive borrows the state one synchronous
  step at a time; every callback from generated C reaches it through the active realm and a
  `try_borrow_mut`. `ACTIVE_RUNTIME`, `ActiveRuntime` and every `as_ptr()` into the runtime are
  gone, so a re-entrant borrow now fails with `RUNTIME_ERROR` instead of aliasing.

The drive does not hold one `&mut RealmState` across its awaits, and cannot: Hyper tasks run the
language while the drive is parked, and the language calls back into the state. What it holds for
its lifetime is exclusive ownership of the cell; the borrow checker's part of the argument is that
no path to the state bypasses it.

Measured on the ten-line server (`bench/server/serve.mjs`, `native-release`): one
`kiln_realm_enter` and one `kiln_realm_drive` for 2,000 requests, so executor entries per request
are 1/2,000 (counted with breakpoints). Tokio's `park_count` is about 2 per request before and
after, and throughput is unchanged within noise (c=16: 14,156 / 15,168 rps before, 14,983 /
14,702 after, two clean interleaved rounds). The small service (`bench/server/app.mjs` under
`drive-mix.mjs`, c=16) costs the same server CPU per request on both sides, within the noise of a
machine at load average 13 to 17 (61.7 / 50.4 / 49.0 µs before, 62.2 / 52.5 after in the rounds
not disturbed by other work). The throughput this ADR was after was earned by the
earlier steps; this one is the structural half, and the first step of ADR 0070: the state is now
an owned value any executor can drive, apart from the HTTP tasks already spawned on one.

