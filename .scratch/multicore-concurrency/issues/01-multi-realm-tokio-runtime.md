# Issue 01: Multi-Realm In-Process Hosting on Tokio Worker Threads

Status: planned
Feature: multicore-concurrency
Governing ADR: ADR 0028, ADR 0057

## Context
ADR 0028 establishes that each Realm is owned by exactly one local task on a current-thread Tokio runtime, and that additional runtime threads create additional current-thread runtimes rather than moving a live realm. Today, Kiln uses multi-process scaling (`KILN_WORKERS`), while in-process multi-threading is restricted to virtual-clock agent testing in the interpreter (ADR 0039).

## Requirements
1. Extend `crates/kiln-runtime` to support a pool of background OS worker threads, each running its own current-thread `RealmRuntime`.
2. Allow spawning a fresh `Realm` on a worker thread that shares the compiled binary's `.text` and `.rodata` segments.
3. Keep heap state completely disjoint: each realm has its own global object, shape table, and turn-boundary GC state.
4. Provide an asynchronous message-passing queue (`tokio::sync::mpsc`) for inter-realm communication.

## Acceptance Criteria
- Spawning a worker realm takes < 1 ms and adds < 2 MB resident memory.
- Crates `kiln-runtime` and `kiln-host-abi` compile cleanly with `mbx test -p kiln-runtime`.
- No cross-realm pointers exist between disjoint heaps.
