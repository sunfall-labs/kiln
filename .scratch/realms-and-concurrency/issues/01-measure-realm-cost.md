# 01. Measure what a realm costs and what state is process-global

Status: ready-for-agent

Two numbers ADR 0070 rests on, measured before anything changes.

1. **What a realm costs.** For a hello-world native binary and the small-service benchmark: resident
   memory broken down into the bootstrap heap (built-ins, intrinsics, module namespaces), the
   Tokio runtime and its buffers, the HTTP state, and everything else; and the time from process
   start to the first line of user code. Use the SIGUSR1 heap report (`runtime/heap.c`) and the
   Linux profiling setup. The bootstrap heap is what the shared immutable region (08) removes from
   every realm after the first.
2. **What state is process-global.** An inventory of every mutable static in
   `crates/kiln-backend-c/src/runtime/*.c` (about 45 on 2026-09-26), every kind of static the
   emitter writes into generated code (about 57), `kiln_namespaces`, and every Rust `static` or
   `thread_local!` in `kiln-runtime` and the provider. For each: what it holds and whether it is
   realm state, a cache, or genuinely process-wide.

## Acceptance

- Both results written into this file's comments, with the commit measured.
- ADR 0070's "How cheap a realm must be" section updated with the starting point.
