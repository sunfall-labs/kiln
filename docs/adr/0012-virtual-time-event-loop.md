# ADR 0012: The event loop runs on a virtual clock the program cannot observe

## Status

Accepted.

## Context

design.md §11 asks for a mock event driver with virtual time, and sketches the shape it should
take:

```javascript
const timer = setTimeout(callback, 1000);
testRuntime.advanceTime(1000);
```

The stated goals are what matter: no real sleeping, exact callback-order assertions, repeatable
race tests, and easy failure minimization. The `testRuntime` handle is one way to reach them, not
the goal itself.

Two facts pushed against implementing that shape directly.

The first is that timers are not only a testing concern. `setTimeout` is ordinary JavaScript, and
the conformance corpus contains a probe that uses it alongside `setInterval`, `clearTimeout` and
`queueMicrotask`. A scheduler that exists only inside a test runtime would leave `kiln run` unable
to execute a program the corpus already asks it to execute, and a second real scheduler for the
CLI would be exactly the divergence §11 exists to avoid.

The second is that `advanceTime` needs somewhere to live. Kiln's execution entry points are
one-shot functions -- `execute` and `execute_module` build a machine, run it, and return a result.
There is no handle a test could hold between statements, and inventing one purely to expose
`advanceTime` would put a lifetime on the public API in exchange for a capability nothing yet
needs.

## Decision

Virtual time is a property of the machine, not of a test harness. The interpreter holds a clock in
milliseconds, a set of scheduled callbacks, and a microtask queue. `setTimeout`, `setInterval`,
`clearTimeout`, `clearInterval` and `queueMicrotask` are the entry points, and they are dispatched
through the interpreter rather than the pure builtin path because scheduling is a machine
operation.

The clock advances only when a timer is due, and only to that timer's due instant. Nothing reads
the host clock. Two consequences follow, and both are the point:

- A program that schedules an hour out finishes instantly.
- A program's callback order is a function of the program alone, so it is the same on every run
  and on every machine.

Execution finishes when the queues are empty rather than when the last statement is: a program is
not done while it still has work scheduled, which is what makes a timer observable at all.

Ordering follows the language rather than the queue's internal shape. Microtasks drain completely
before any timer and again after each one. Timers due at the same instant run in the order they
were scheduled, which needs a sequence number because a due time is not a total order. An interval
is rescheduled *before* its callback runs and under the same id, so a `clearInterval` from inside
cancels the next run rather than a run that no longer exists.

`advanceTime` is deliberately not exposed yet. Draining to quiescence is the same operation with
the bound at infinity, and every test written so far asserts an *order* rather than a state at an
instant. When a test genuinely needs to stop half way -- to assert that a callback has *not* run --
that is the point to add a persistent machine handle, and the clock is already the only thing it
would need to control.

## Consequences

Async tests are fast and repeatable, and a race is reproducible because there is no race: the
order is decided by the program.

The differential harness cannot check the part that matters most. `scripts/differential.sh` diffs
against node, and node sleeps for real, so a probe with an hour-long delay would hang the reference
implementation rather than demonstrating the difference. Timer probes there therefore use delays of
a few milliseconds and assert ordering only. The virtual clock's own behaviour -- that the delay's
*magnitude* costs nothing -- is covered by unit tests instead, where there is no reference
implementation to wait for.

An infinite `setInterval` that is never cleared does not hang: every callback costs steps, so the
existing step limit bounds it. That is a weaker guarantee than a task-count limit and a sharper
diagnostic would be better, but it is a real bound rather than an assumed one.

Nothing here makes the *host* clock available. `Date.now` remains unimplemented, and when it
arrives it has to decide whether it reads this clock or the real one -- reading the real one would
reintroduce exactly the nondeterminism this removes.
