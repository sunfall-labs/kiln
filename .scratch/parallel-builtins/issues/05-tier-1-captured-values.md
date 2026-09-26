# 05. Tier 1: read captured values

Status: ready-for-agent
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
