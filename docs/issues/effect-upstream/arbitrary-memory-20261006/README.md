# Arbitrary interpreter memory failure — 2026-10-06

The full interpreter survey used frozen CLI `148946a23bb17b60b6d1f5907364c8950c752992c09949d1eeb287b3bdec4d4b`, source `efd9e68a8920d764ccc7df1f244d7591d74794d6`, unchanged upstream TypeScript and original case deadlines. The runner limits each child to a 4 GiB address space; the survey service also has a 4 GiB resident-memory cap and 2 GiB swap allowance. This receipt concerns one terminal child; the overall survey continues.

`arbitrary/Arbitrary` registered 178 cases and emitted only 172 outcomes: 165 pass, 7 fail on original 5,000 ms deadlines, and no skips. It exited by SIGABRT after 106.85 seconds, without a summary. The complete raw log ends with `memory allocation of 16777216 bytes failed`. Six registered cases therefore remain unrecorded; this file is **incomplete**, not a passing or completed conformance result. The envelope retains the exact commands and Node/Vitest mapping.

The last emitted pass is `lazily materializes structural shrinks and replays them`. The next registered case is `traverses a thousand lazy child shrinks without overflowing`, which builds a thousand-element input and evaluates 1,001 shrinks. The failure is consistent with the imposed address-space limit; the log alone does not distinguish cumulative retention from that case's peak working set.

A source audit found `collect_closure_cycles()` is invoked in production only by `Native::GcRequest`. Weak-only allocation censuses can prune expired Rc records but cannot release disconnected strong-reference cycles. Automatic collection under allocation pressure is being investigated, with active-stack/realm roots and same-job WeakRef keepalive requirements preserved. No memory repair or deadline improvement is claimed by this evidence.
