# PRD: Rust-Backed Native Concurrency and Runtime Engine

## Status: Proposed
## Champion: Antigravity & User
## Governing ADR: ADR 0018, ADR 0025, ADR 0057, ADR 0058

---

## 1. Executive Summary & Philosophy
In Kiln's architecture, emitted C is exclusively a thin, syntax-directed translation unit. Substantive runtime capabilities, memory management, data structures, and concurrency engines belong in **Rust**, compiled into the native staticlib (`libkiln_native_number_ffi.a`) and linked by Zig/Clang into every native binary.

This project moves the execution of data-parallel array operations (`parallelMap`, `parallelReduce`, `parallelSort`) from single-threaded scalar C loops into a high-performance **Rust Rayon work-stealing compute engine**, exposed via the native provider FFI ABI using Chunked Loop Inversion to achieve zero per-element FFI overhead.

---

## 2. Architecture & Link Seams

```
+-------------------------------------------------------------------+
|                        Emitted Native C Program                   |
|   arr.parallelMap(fn)                arr.parallelReduce(init, fn) |
|   arr.parallelSort()                                              |
+---------------------------------+---------------------------------+
                                  | extern "C" ABI (once per chunk)
                                  v
+-------------------------------------------------------------------+
|              Rust Staticlib (libkiln_native_number_ffi.a)         |
|  - kiln_parallel_map                                              |
|  - kiln_parallel_reduce                                           |
|  - kiln_parallel_sort                                             |
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
   - `parallelMap`, `parallelReduce`, and `parallelSort` leverage available CPU cores in compiled native C executables via work-stealing.
2. **Chunked Loop Inversion**:
   - C inlines and auto-vectorizes (AVX2/NEON) inner loop chunk workers; Rust coordinates chunks across threads ($<0.001\%$ FFI cost).
3. **First-Class Rust Implementation**:
   - Algorithms, synchronization, chunking, and worker scheduling are implemented in Rust. Emitted C is solely the caller.
4. **Provider ABI Contract Evolution**:
   - Register `kiln_parallel_map`, `kiln_parallel_reduce`, and `kiln_parallel_sort` in `kiln-native-number-abi`. Bump `PROVIDER_ABI_VERSION` from 29 to 30.
5. **Preserve Shared CPU Budget**:
   - Use a private, bounded `rayon::ThreadPool` rather than the unbound global pool, adhering to `AGENTS.md` and `NOTES.md`.

---

## 4. Implementation Issues
- **`01-native-provider-abi-extension.md`**: ABI registry, exports, version bump (29 -> 30), and packager contract verification.
- **`02-rust-rayon-compute-pool.md`**: Work-stealing Rayon compute pool and chunk scheduler in `kiln-runtime` / `kiln-native-number-ffi`.
- **`03-c-backend-chunk-lowering.md`**: Backend chunk worker emission in `kiln-backend-c` routing to Rust exports.
- **`04-concurrency-surface-and-effect.md`**: TypeScript `@kiln/concurrency` and `@kiln/effect` surface for `parallelReduce` and `parallelSort`.
- **`05-conformance-and-benchmarking.md`**: Conformance test cases, native probes, and multicore MapReduce benchmarks.
