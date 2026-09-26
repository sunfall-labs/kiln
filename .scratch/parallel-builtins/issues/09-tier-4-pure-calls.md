# 09. Tier 4: pure calls

Status: ready-for-agent
Blocked by: 07 (08 for calls that allocate)

1. **Pure built-ins.** Add a "pure" mark to the member description for built-ins with no side
   effects and no user-code callbacks for plain-data arguments: `Math`, string methods, number
   formatting (`toFixed`, `toString`), `JSON.stringify` of plain data, `Array.prototype.slice`,
   and so on. A parallel callback may call a marked built-in.
2. **Pure user functions.** Extend the proof interprocedurally: a callback may call a user function
   whose own graph satisfies the same rules, to a bounded depth, with recursion allowed when every
   function in the cycle is proven.

## Acceptance

- `lines.parallelMap(l => l.trim().split(",").map(Number))` runs in parallel.
- A call to an unmarked built-in or an unproven function keeps the call sequential, naming it.
