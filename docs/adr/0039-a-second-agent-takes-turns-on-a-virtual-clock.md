# ADR 0039: A second agent takes turns on a virtual clock, and needs no concurrency crate

## Status

Accepted.

Applies ADR 0029's adoption rule to the agent scheduler in `kiln-interpreter`, and extends ADR
0012's virtual clock from one realm's timers to the ordering between agents. It does not change
ADR 0028's runtime decision: it uses the clause that already says additional runtime threads
create additional current-thread runtimes rather than moving a live realm.

It also needs ADR 0031's per-realm intrinsics, which is what makes a second agent's realm
constructible on its own terms rather than as a copy of the first's.

## Context

Test262 has a host protocol for a second *agent*. `$262.agent.start(src)` runs `src` in an
execution context with a realm of its own; the two communicate only through a
`SharedArrayBuffer`, `Atomics`, and a report queue. About 218 variants depend on it, and they are
the reason `Atomics.wait` and `Atomics.notify` existed in Kiln without being able to do anything:
with one agent, nothing can ever reach the waiter list, so `wait` answered "not-equal" or
"timed-out" and `notify` woke nobody. That was the honest answer. It was also a ceiling.

Two things make this harder than "spawn a thread".

The first is that a realm's heap is `Rc`-based and not `Send`. It cannot move between threads and
cannot be shared. Only a `SharedArrayBuffer`'s bytes may cross, which is exactly what the language
says about them.

The second is determinism, and it is the one that decided the design. Test262's own agent tests
are written against wall-clock races and say so:

```js
// There's a slight risk we'll fail to notify the desired count, if the preceding
// tryYield() took much longer than anticipated and workers have started timing out.
assert.sameValue(Atomics.notify(i32a, 0, NOTIFYCOUNT), NOTIFYCOUNT, ...);
```

`notify-one.js` starts three agents, sleeps 100ms in the hope that all three have reached
`Atomics.wait`, and then asserts that `Atomics.notify(i32a, 0, 1)` returns exactly 1. In an engine
whose agents are scheduled by the operating system, that assertion holds most of the time. ADR
0020 says a conformance number has to mean something, and a suite that answers differently on a
loaded machine is not a measurement; a flaky agent test is worse than a refused one, because it is
diagnosed as a real regression weeks later by somebody else.

There is also a failure mode sharper than a wrong answer. A real `Atomics.wait` with no timeout
and no notifier is a hang, and a hang in a corpus sweep is not one failing test — it is a
147-chunk run stopped by a per-chunk timeout.

## Decision

### Agents are threads, but they take turns

Each agent is an `std::thread` with its own current-thread `RealmRuntime` and its own realm. That
is what ADR 0028 already prescribes for a second runtime thread, so no runtime decision changes:
the main agent's realm stays on the runtime that owns it, and the new one gets its own rather than
being moved onto or spawned into the first.

Exactly one agent runs at a time. A baton passes when the running agent blocks — `Atomics.wait`,
`$262.agent.sleep`, waiting on a broadcast — or when it has executed a fixed number of bytecodes
without blocking. The second half is what lets a spin loop make progress: Test262's own
`$262.agent.waitUntil` is `while (Atomics.load(ta, i) !== n) {}` with no blocking call in it, and
a scheduler that only switched on blocking would spin there for ever.

The preemption point is the comparison the dispatch loop already made. `Vm::checkpoint` is
`min(max_steps, end of this quantum)`, so a program with no second agent pays exactly the
step-limit check it always paid, and the slow path decides which of the two boundaries arrived.

### Time between agents is virtual, by ADR 0012's rule

The group has one clock. `$262.agent.sleep`, `Atomics.wait` timeouts, `Atomics.waitAsync`
deadlines and `$262.agent.monotonicNow` all read it, and it advances only when no agent can make
progress — and then only as far as the earliest deadline. This is ADR 0012's rule verbatim, moved
up one level: there it ordered one realm's timers, here it orders the agents against each other.

Consequently a 10-second `Atomics.wait` costs no wall-clock second and the whole 778-variant
`Atomics` subtree runs in 167ms.

One extension is needed. A *spinning* agent is progressing by that rule, so two agents spinning on
a cell that only a sleeping third agent will ever write would hand the baton back and forth for
ever while the sleeper's deadline never arrived — and `notify-in-order.js` is exactly that
program. After a fixed number of consecutive quanta in which no agent blocked, woke or left, the
clock is allowed to move to the earliest deadline anyway. It is a constant, so the interleaving
remains a function of the program alone.

### A deadlock is answered, not endured

When no agent is runnable and nothing has a deadline, the program has genuinely deadlocked. Every
waiter is released with "timed-out". That is not the truth — a wait with no timeout did not time
out, it will simply never end — but it is the same answer this engine already gave a single agent
waiting on a buffer nobody else could reach, and it is the only answer that is not a hung process.
The alternative is a sweep that stops rather than a test that fails.

Agent threads are joined when the main agent's machine is dropped, on every path out including an
unwind, because a thread that outlived its case would be a thread holding a whole realm and a
conformance process runs tens of thousands of cases.

### `std::sync` is the dependency, and that is the decision

**No concurrency crate is adopted.** `parking_lot` and `crossbeam` were both considered against
ADR 0029's adoption rule and both are refused.

**`parking_lot`.** Its futex-backed `park`/`unpark` maps almost exactly onto `Atomics.wait` and
`Atomics.notify`, and that is the reason it is wrong here rather than the reason it is right. A
futex hands the wake decision to the operating system. The wake decision is precisely the thing a
conformance run has to make for itself: `Atomics.notify(i32a, 0, 1)` must answer 1 because the
schedule put one waiter on that cell, not because the kernel happened to have parked one there in
time. Adopting it would mean building the deterministic scheduler anyway and then using the crate
for the one primitive the scheduler makes unnecessary. Criterion 6 — the deletion test — fails in
reverse: removing it would remove nothing.

Its faster uncontended mutex is not a reason either. The baton means there is never contention:
the only writer of a shared buffer's bytes is the one agent holding the baton.

**`crossbeam`.** Its channels would carry the report queue and the broadcast mailboxes. Both
already live inside the single mutex that orders every other decision the scheduler makes, and a
channel would add a second synchronisation point with an ordering of its own to keep honest
against the first. Criterion 1 fails: there is no seam here, only a `VecDeque` behind a lock that
is held anyway.

**`loom`.** Worth naming because ADR 0029 §3 already earmarks it for exactly this kind of module.
It is not adopted *yet* and the reason is that the scheduler is single-baton: there are no
interleavings for it to explore that the deterministic schedule does not already fix. If the baton
is ever relaxed — genuinely parallel agents behind a lock on the shared bytes — that is the moment
`loom` earns its place, and this paragraph is the note to that effect.

What is left is a condition variable guarding one piece of state, which `std::sync` has. It costs
nothing on ADR 0004's rebuild path, which matters because `kiln-interpreter` is on it.

## Consequences

- `built-ins/Atomics` goes from 530 to 762 of 778 passing, and the 12 that still fail are exactly
  the 12 that failed before agents existed. The 224 variants that were `unsupported` are 0.
- Two full corpus sweeps agree chunk for chunk, which is the property this design exists for.
- `SharedArrayBuffer` bytes now live behind `Arc<Mutex<Vec<u8>>>` rather than `RefCell<Vec<u8>>`.
  Ordinary buffers are unchanged. Every reader still spells the access `borrow`/`borrow_mut`,
  because reading a buffer's bytes is one operation whoever holds them.
- The interleaving is a function of the program *and of two constants* — the quantum and the spin
  patience. Changing either changes which schedules the corpus exercises, so they are part of the
  engine's observable behaviour in the same way the step limit is, and moving them requires a
  sweep.
- Agents are capped per program. Each is a thread and a Tokio runtime, and a program asking for
  ten thousand would take the machine down rather than fail, which ADR 0020 counts as worse than
  refusing.
- The C backend refuses all eight `$262Agent*` hooks by name. It emits one translation unit with
  one global object and no scheduler, so a compiled program reaching them would answer
  "timed-out" where the interpreter answered "ok" — a silent divergence, which is the one thing
  `scripts/native.sh` exists to prevent.

## Alternatives rejected

**Real threads on the wall clock.** The smallest change, and the one every other engine makes.
Rejected because `notify-one.js` and its dozen siblings assert a count that depends on whether
three agents reached `Atomics.wait` within 100 milliseconds. It would pass on an idle machine and
fail under a three-way sweep, and a conformance number that moves with the machine's load cannot
be used to decide anything.

**Agents as coroutines on one thread.** No threads, therefore no `Send` problem and no scheduler
to write. Rejected because the interpreter cannot be suspended at an arbitrary instruction and
resumed later: `run_bounded` re-enters itself whenever a built-in calls back into JavaScript, so
an agent's state is partly a Rust stack. A thread is a way to keep that stack while giving up the
processor, and a baton makes it behave like a coroutine anyway.

**Keep refusing.** `$262.agent` refusing by name is honest and costs nothing, and it was the right
answer for as long as `Atomics.wait` could not block. It stops being the right answer once the
alternative is 232 variants and a genuine implementation of a language feature the engine already
half had.
