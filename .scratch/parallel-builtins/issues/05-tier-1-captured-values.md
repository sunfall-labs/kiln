# 05. Tier 1: read captured values

Status: resolved
Blocked by: 02

Today rule 4 of `parallel_safe` refuses any upvalue, "even a numeric one", because an upvalue is a
heap cell read. Allow reading a captured variable when the compiler proves it is never written
after the callback is created (a `const`, or a `let` with no later assignment reachable), and the
value read is a number, boolean, `null` or `undefined`. The cell is read once on the realm thread
before the pool starts and passed to workers as a parameter, so workers still touch no heap.

This tier needs no change to the collector, which is why it comes first.

## Acceptance

- `xs.parallelMap(x => x * rate)` with `const rate = 1.2` runs in parallel.
- A captured `let` reassigned anywhere after the callback is created keeps it sequential, with
  the reason from 03.

## Comments

2026-09-26, branch `alee/parallel-tier1`.

**Which captures.** `direct.rs`'s whole-unit binding walk now also answers, per proto and
upvalue, how many writes the unit has of the binding the upvalue resolves to
(`parallel::Capture::Writes(n)`), not counting the `undefined` a declaration hoists ahead of its
first real write in the owner's entry block; a chain it cannot resolve, or a unit with a write it
cannot place, is `Capture::Unresolved`. Rule 4 of `parallel_verdict` admits a `LoadUpvalue` of a
binding written exactly once (`const`, a `let`/`var` assigned once, a parameter); otherwise
`captures-reassigned` (with the count) or `captures-unresolved`. `StoreUpvalue` is
`writes-capture`. Being conservative costs nothing in soundness: while the call runs the value
cannot change at all (the callback writes nothing, the realm thread is blocked in the call), so
the snapshot is exact; the write-once rule is ADR 0069's policy.

**The proof.** The plan proves the return a number with the parameters *and the captured
reads* seeded as numbers, in a separate parallel plan per qualifying proto (`DirectPlan::parallel`),
from which the `_p` clone is emitted; the `_t` typed entry is unaffected. Only protos some
`parallelMap` site is known to be called with get a clone at all.

**How the values reach workers.** The clone takes each captured binding as a `double cap<k>`
parameter after its own and reads it in place of the cell (`LoadUpvalue` under the clone flag).
At the call site, after the closure is known to be the target, the realm thread reads each cell
from `plain->upvalues[k]` and requires `KILN_VAL_IS_NUMBER`; the values go into `struct { int
failed; double caps[N]; }`, passed as the existing `ctx` (the flag stays at offset 0, so the
chunk workers' failure protocol is unchanged), and each chunk worker passes `caps[j]` to the
clone. If any capture is not a number -- or is still in its dead zone -- nothing is allocated and
the sequential lowering answers, throwing what `map` throws. Workers still touch no heap, and the
clone still passes `lib.rs`'s text check (no `upvalues`, `ctx`, host, heap).

**Numbers only.** A captured boolean, `null` or `undefined` compiles as tier 1 but takes the
sequential path at run time: passing them as doubles would change `===` and `typeof` inside the
callback. Widening that means typed lanes for them in the clone.

**Acceptance.** `xs.parallelMap(x => x * rate)` with `const rate = 1.2` runs in parallel
(`parallel-capture-const.js:24:19  parallelMap  parallel (tier 1: reads captured values)`); a
captured `let` assigned again is sequential with `captures-reassigned`, naming the binding and the
write count. Pinned by `parallel.rs` (`tier_1_a_capture_written_once_qualifies`,
`reason_captures_*`, `reason_writes_capture`), `parallel_tests.rs`
(`tier_1_a_capture_written_once_is_parallel_and_one_written_twice_is_not`, and captures in the
property test), and the three `parallel-capture-*` probes at `KILN_PARALLELISM=1`, `2`, `max`.
