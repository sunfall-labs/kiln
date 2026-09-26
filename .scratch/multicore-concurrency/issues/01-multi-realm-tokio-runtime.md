# Issue 01: Multi-Realm In-Process Hosting on Tokio Worker Threads

Status: resolved
Feature: multicore-concurrency
Governing ADR: ADR 0028, ADR 0057

## Context
ADR 0028 establishes that each Realm is owned by exactly one local task on a current-thread Tokio runtime, and that additional runtime threads create additional current-thread runtimes rather than moving a live realm. Today, Kiln uses multi-process scaling (`KILN_WORKERS`), while in-process multi-threading is restricted to virtual-clock agent testing in the interpreter (ADR 0039).

## Requirements
1. Extend `crates/host/kiln-runtime` to support a pool of background OS worker threads, each running its own current-thread `RealmRuntime`.
2. Allow spawning a fresh `Realm` on a worker thread that shares the compiled binary's `.text` and `.rodata` segments.
3. Keep heap state completely disjoint: each realm has its own global object, shape table, and turn-boundary GC state.
4. Provide an asynchronous message-passing queue (`tokio::sync::mpsc`) for inter-realm communication.

## Resolution
Landed 2026-09-13 in `crates/host/kiln-runtime/src/worker_pool.rs`:
- Implemented `WorkerPool` and `WorkerRealmHandle` hosting isolated `RealmRuntime` instances across dedicated OS worker threads.
- Added support for host factory initialization (`spawn_realm_with_factory`) ensuring `!Send` hosts such as `TokioHost` are constructed and run strictly on the worker thread.
- Bounded `tokio::sync::mpsc` mailbox queue (`DEFAULT_MAILBOX_CAPACITY = 1024`) with `oneshot` response channels for non-blocking task execution and stateful actor dispatch.
- Implemented `ActorHandler` trait and `ActorRegistry` supporting stateful in-realm actors with boundary-safe `HostValue` and `HostError` parameters.
- Protected against panics via `catch_unwind` and panic-safe `WorkerDropGuard` tracking active worker counts.
- Verified: `mbx test -p kiln-runtime` (128 passed, 0 failed in 0.08s) including fast spawning benchmark, disjoint heap isolation, actor registration and invocation, and capacity limits.
