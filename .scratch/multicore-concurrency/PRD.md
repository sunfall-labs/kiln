# Multicore In-Process Concurrency: Native Actors, Zero-Copy Region Transfer, and Effect v4 Integration

Status: in-progress

Governing ADRs: [ADR 0057](file:///home/coder/kiln/docs/adr/0057-region-isolated-actors-and-zero-copy-transfer.md), [ADR 0028](file:///home/coder/kiln/docs/adr/0028-one-tokio-runtime-owns-interpreted-and-native-execution.md), [ADR 0041](file:///home/coder/kiln/docs/adr/0041-the-native-heap-is-collected-at-the-turn-boundary.md), [ADR 0056](file:///home/coder/kiln/docs/adr/0056-value-graph-and-verifier.md).

---

## Why

Today, Kiln achieves multicore server throughput via `KILN_WORKERS=auto`, which forks independent operating system processes over an `SO_REUSEPORT` listener ([README.md](file:///home/coder/kiln/README.md#L68-L77)). On Linux x86_64, four workers serve 107.8k requests/sec while holding under 10 MB resident memory per process.

However, in-process multicore coordination (e.g. shared caches, background CPU compute, multi-core data pipelines, and Effect v4 fibers) currently faces three flawed industry alternatives:
1. **Web Workers / `node:worker_threads`:** Deep serialization via `StructuredClone` incurs tens of milliseconds of CPU latency when passing large graphs, doubling peak memory.
2. **Unconstrained Shared Heap (Bun PR #249 `new Thread(fn)`):** Pthreads-style shared mutable heap causes data races on internal shape trees (ADR 0050), breaks Secure ECMAScript (SES) boundaries, forces atomic locks onto every single-threaded property read, and is completely incompatible with Kiln's turn-boundary GC (ADR 0041).
3. **TC39 Shared Structs (Stage 2):** Solves memory safety by restricting shared state to fixed layouts, but forces developers into 1995-era manual locking (`Atomics.Mutex.lock()`, `Atomics.Condition`), introducing deadlock hazards and breaking standard TypeScript idioms.

### Grounding in Modern PL Research
Recent breakthroughs in language design offer a fundamentally superior path:
* **Pony's Deny Capabilities (OOPSLA 2015):** Proves that an isolated object reference (`iso`) with no external aliases can be transferred across actors via an $O(1)$ pointer handoff with zero copying, zero serialization, and zero locks.
* **Swift 6 Region-Based Isolation (Apple, 2024; SE-0414):** Proves that isolation does not require complex lifetime types; the compiler can infer disconnected regions directly from control-flow escape analysis.
* **Project Verona (Microsoft Research, 2020–2022):** Memory is organized into isolated subgraphs (regions) where allocations and mutations remain lock-free and thread-local.
* **Lean 4 / Perceus (ICFP 2020, PLDI 2021):** Proves that functional, immutable data structures can achieve C-level performance via in-place destructive updates when reference counts are unshared.

By combining Kiln's ahead-of-time (AOT) C compiler with its Rust/Tokio runtime, Kiln can achieve **zero-copy in-process multicore parallelism without manual mutexes or serialization latency**.

---

## Scope

1. **Engine Foundation (`crates/kiln-runtime` & `crates/kiln-backend-c`):**
   * Multi-realm hosting on Tokio worker threads: background threads run disjoint `Realm` instances sharing the native binary's compiled `.text` and `.rodata` segments (~1–2 MB RSS per worker).
   * $O(1)$ GC block splicing: unlinks a doubly-linked `kiln_c99_gc_block` chain from Realm A and splices it into Realm B's heap header (< 5 nanoseconds, zero bytes copied).
   * Native work-stealing loop (`__kiln_parallel_for`) over Tokio/Rayon worker pools.
2. **Base TypeScript Surface (`@kiln/concurrency` in `crates/kiln-builtin-surface`):**
   * `Actor` base class with mapped `ActorRef<T>` proxy types (automatically wrapping synchronous methods in `Promise<Awaited<R>>`).
   * `isolate<T>()` and `transfer<T>()` using phantom nominal brands for static transfer verification.
   * `Array.prototype.parallelMap()`.
   * **100% `tsc --strict` compatibility:** No compiler plugins, Babel transforms, or non-standard syntax.
3. **Effect v4 Universal Library (`@kiln/effect`):**
   * `KilnWorkerPool.layer`: Drop-in replacement for `@effect/experimental/WorkerPool`, farming out tasks to in-process Tokio Realms via $O(1)$ GC block splicing.
   * `KilnNativeScheduler`: Implements Effect's `Scheduler` interface, driving fibers on Tokio's M:N work-stealing pool and linking fiber interruption tokens to native atomic cancellation flags.
   * Dual-mode portability: Runs via native host-ops in Kiln, with automatic fallback to `node:worker_threads` when running in Vitest/Node.
4. **Compiler Passes (`crates/kiln-hir` & `crates/kiln-backend-c`):**
   * Region escape analysis: Proves that expressions passed to `isolate()` have no remaining aliases in the local lexical scope.
   * Purity verification: Checks that closures passed to `parallelMap()` and pure `Effect.forEach()` have no mutable outer-scope captures (`Effect::Pure` in ADR 0056).
   * Direct C11 loop lowering: Compiles pure parallel iterations directly to native scalar C work-stealing loops.
5. **Conformance & Benchmarking:**
   * Test suites in `conformance/effect/src/` and `conformance/native/`.
   * Multi-core server throughput and resident memory benchmarks under `bench/server/`.

---

## Non-Goals & Invariants

* **No Shared Mutable Heaps:** Two OS threads never touch the same heap block concurrently. Turn-boundary GC (ADR 0041) remains 100% single-threaded and lock-free per realm.
* **No Compiler AST Magic for Effect:** The engine remains general-purpose. Effect integration is implemented as an explicit, dual-mode library (`@kiln/effect`) implementing Effect's standard SPIs, not via speculative AST rewrites.
* **Tooling Parity:** All user-facing concurrency code must typecheck cleanly under vanilla `tsc --strict --target es2022`.

---

## Issues & Implementation Roadmap

| # | Issue Slug | Title | Status |
|---|---|---|---|
| [01](issues/01-multi-realm-tokio-runtime.md) | `multi-realm-tokio-runtime` | Multi-Realm In-Process Hosting on Tokio Worker Threads | planned |
| [02](issues/02-c99-gc-block-splice.md) | `c99-gc-block-splice` | $O(1)$ GC Block Sublist Unlinking and Splicing in Native C | planned |
| [03](issues/03-base-concurrency-surface.md) | `base-concurrency-surface` | Base TypeScript Concurrency Surface (`@kiln/concurrency`) | planned |
| [04](issues/04-effect-v4-worker-pool-and-scheduler.md) | `effect-v4-worker-pool-and-scheduler` | Universal Effect v4 Integration (`@kiln/effect`) | planned |
| [05](issues/05-hir-escape-and-purity-analysis.md) | `hir-escape-and-purity-analysis` | HIR Escape Analysis & SSA Purity Loop Lowering | planned |
| [06](issues/06-conformance-and-benchmarking.md) | `conformance-and-benchmarking` | Conformance Test Suite & Multicore Performance Benchmarking | planned |

---

## Comments & Review History

* **2026-09-06:** Initial PRD drafted following architectural review of Jarred Sumner's Bun PR #249, TC39 Structs Stage 2, and PL research literature (Pony, Swift 6, Verona, Lean 4). [ADR 0057](file:///home/coder/kiln/docs/adr/0057-region-isolated-actors-and-zero-copy-transfer.md) accepted as the governing specification. `tsc --strict` typechecking contract verified on synthetic test cases.

* **2026-09-06 (review):** All six issues were marked done by commit 555de594, but none of the three tiers runs: no worker realm is reachable, no GC splice exists, and `parallelMap` lowers to a sequential loop. Statuses reset to `planned` with a reality-check line in each issue. See ADR 0058 (revised) for the gate a parallel lowering must pass.
