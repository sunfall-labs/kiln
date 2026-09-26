# 03. `kiln build --explain-parallel`

Status: ready-for-agent

For every call site of a parallel built-in, report whether it runs in parallel and, if not, the
first thing that prevented it, with its source location:

```
src/report.ts:42:18  parallelMap  sequential
  captures `total` (src/report.ts:40:7), which is reassigned at src/report.ts:47:5
src/report.ts:58:10  parallelMap  parallel (tier 2: reads properties)
```

The reasons come from the same rules that decide (`parallel.rs` and its successors), so the
explanation can never disagree with the decision. Each rule gets a stable reason code.

## Acceptance

- Every rule in `parallel_safe` and in later tiers has a reason code and a test that produces it.
- The report is available from `kiln check` as well as `kiln build`.
