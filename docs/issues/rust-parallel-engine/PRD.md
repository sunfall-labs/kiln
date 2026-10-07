# PRD: Rust-Backed Native Concurrency and Runtime Engine

## Status: resolved (landed 2026-09-06; ADR 0058 application 2)
## Champion: Antigravity & User
## Governing ADR: ADR 0018, ADR 0025, ADR 0057, ADR 0058

---

## 1. Executive Summary & Philosophy
In Kiln's architecture, emitted C is exclusively a thin, syntax-directed translation unit. Substantive runtime capabilities, memory management, data structures, and concurrency engines belong in **Rust**, compiled into the native staticlib (`libkiln_native_number_ffi.a`) and linked by Zig/Clang into every native binary.

This project moves `parallelMap` from a single-threaded scalar C loop into a **Rust Rayon fork-join** in the provider archive, exposed through one export whose chunk worker is an emitted static C function, so the FFI is crossed once per chunk. `parallelMap` is synchronous and returns an array; the pooled lowering is taken only for a callback the backend proves allocates nothing and touches no heap object but its input, and every other callback is refused with the reason named (ADR 0058, revised 2026-09-06). `parallelReduce` and `parallelSort` are not part of the decision.

---

## 2. Architecture & Link Seams

```
+-------------------------------------------------------------------+
|                        Emitted Native C Program                   |
|   arr.parallelMap(fn)   -- synchronous, gated on an allocation-free callback   |
|                                                                   |
+---------------------------------+---------------------------------+
                                  | extern "C" ABI (once per chunk)
                                  v
+-------------------------------------------------------------------+
|              Rust Staticlib (libkiln_native_number_ffi.a)         |
|  - kiln_parallel_map (one export; a sequential floor below which it runs inline)|
|                                                                   |
|                                                                   |
+---------------------------------+---------------------------------+
                                  |
                                  v
+-------------------------------------------------------------------+
|                    Kiln Compute Engine (Rust)                     |
|  - Rayon Work-Stealing Pool (bounded CPU budget)                  |
|  - Adaptive Chunking Heuristic (cache-line aligned)               |
|  - Zero-Allocation Slice Partitioning                             |
|  - Tokio Actor Integration (non-blocking async delegation)        |
+-------------------------------------------------------------------+
```

---

## 3. Goals & Key Requirements
1. **Full Multicore Scaling in Native Binaries**:
   - `parallelMap` uses available CPU cores in compiled native executables via a Rayon fork-join, for callbacks the backend can prove allocation-free; others are refused, never run sequentially in silence.
2. **Chunked Loop Inversion**:
   - An emitted static C chunk worker over `[start, end)`; Rust coordinates chunks across threads, one FFI crossing per chunk. Auto-vectorization is not claimed: it needs closure inlining and a `double[]` array representation, both separate ADRs.
3. **First-Class Rust Implementation**:
   - Algorithms, synchronization, chunking, and worker scheduling are implemented in Rust. Emitted C is solely the caller.
4. **Provider ABI Contract Evolution**:
   - Register `kiln_parallel_map` in `kiln-native-number-abi` and bump `PROVIDER_ABI_VERSION` by one from whatever the tree is at (31 after c963d4bc).
5. **Preserve Shared CPU Budget**:
   - Use a private `rayon::ThreadPool` sized from `available_parallelism`, overridable the way `KILN_WORKERS` is. The `mbx` CPU budget in `AGENTS.md` governs developer machines, not shipped binaries.

---

## 4. Implementation Issues
- **`01-native-provider-abi-extension.md`**: ABI registry row for `kiln_parallel_map`, version bump, packager contract verification.
- **`02-rust-rayon-compute-pool.md`**: Work-stealing Rayon compute pool and chunk scheduler in `kiln-runtime` / `kiln-native-number-ffi`.
- **`03-c-backend-chunk-lowering.md`**: Backend chunk worker emission in `kiln-backend-c` routing to Rust exports.
- **`04-concurrency-surface-and-effect.md`**: wontfix -- `parallelReduce`/`parallelSort` are out of scope; the `@kiln/concurrency` surface already carries the synchronous `parallelMap` type.
- **`05-conformance-and-benchmarking.md`**: a native probe with a callback the gate must refuse (`-refuses.ts`), one it must accept, and a benchmark large enough (>= 1e6 numeric elements) to clear the fork-join floor.


* **2026-09-06 (reconciliation):** Reconciled 2026-09-06 to ADR 0058 (revised): `parallelMap` only, synchronous, lowered to a pool only for a callback the backend proves allocates nothing, else refused with the reason; no pinned ABI version (the archive was at 31 after the JSON reader, c963d4bc); `parallelReduce` and `parallelSort` are not defined anywhere and are out of scope; simd-json was measured against a scalar Rust reader for JSON and dropped (see `docs/issues/json-tape/NOTES.md`). `is_pure()` in `kiln-backend-c` was deleted with the rest of ADR 0057's scaffolding; the allocation-free predicate this PRD needs does not exist yet and is issue 03's first task.
