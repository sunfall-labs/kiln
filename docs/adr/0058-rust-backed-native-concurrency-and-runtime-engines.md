# ADR 0058: Rust-Backed Native Concurrency and Runtime Engines

## Status

Proposed.

Extends ADR 0018 (native runtime in Rust shipped as prebuilt archive), ADR 0025 (engines linked from Rust), and ADR 0057 (region-isolated actors and data parallelism).

## Context

Kiln's compiled C backend emits standalone C99 translation units linked against a Rust static library archive (`libkiln_native_number_ffi.a`).

While single-threaded scalar code and expression graphs compile directly to C99, substantive runtime capabilities, memory management, data structures, scheduling, and concurrency engines belong in Rust. This separation delivers the memory safety, concurrency primitives, and SIMD ecosystem of Rust while keeping the emitted C layer lean, predictable, and simple.

When array operations like `parallelMap` or `parallelReduce` are evaluated, hand-rolling ad-hoc thread synchronization or single-threaded loops in C abandons the rich Rust ecosystem (Rayon, Tokio, Crossbeam) and violates Kiln's architecture.

However, calling across an FFI boundary per element (e.g. 10,000,000 indirect function calls) introduces branch mispredictions and destroys auto-vectorization.

## Decision

We adopt a **Chunked Loop Inversion** architecture across the private provider C ABI:

1. **Inlined Vectorized C Chunk Workers**: Emitted C inlines closure arithmetic and loop bodies into static chunk functions, enabling Clang/Zig to auto-vectorize inner loops using AVX2 or ARM NEON.
2. **Rust-Managed Work Stealing (Rayon)**: The C binary passes the chunk worker function pointer and array slice to Rust across the provider ABI (`kiln_parallel_map`, `kiln_parallel_reduce`, `kiln_parallel_sort`). Rust's dedicated, bounded Rayon thread pool coordinates adaptive cache-line chunking and work-stealing across CPU cores.
3. **Coarse-Grained FFI Boundary**: The FFI boundary is traversed only once per chunk ($O(\text{cores})$ instead of $O(N)$), reducing FFI overhead to $< 0.001\%$ of execution time.
4. **Provider ABI Contract Evolution**: Register new compute exports in `crates/kiln-native-number-abi`, bumping `PROVIDER_ABI_VERSION` from 29 to 30.

## Consequences

* **Bare-Metal Vectorization + Multicore**: C compiles the vectorized inner loop while Rust manages the multicore scheduling and load balancing.
* **CPU Budget Adherence**: Rayon uses a private, bounded thread pool adhering to `AGENTS.md` and `NOTES.md` conventions, preventing machine thrashing across concurrent development checkouts.
* **Unified Architectural Model**: Validates the principle that complex runtime engines (RegExp, BigInt, Temporal, HTTP, Concurrency) are implemented once in Rust and linked into both the interpreter and native binaries.
