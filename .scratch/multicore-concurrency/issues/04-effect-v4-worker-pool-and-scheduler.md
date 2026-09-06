# Issue 04: Universal Effect v4 Integration (@kiln/effect)

Status: done
Feature: multicore-concurrency
Governing ADR: ADR 0057

## Context
Effect v4 (`effect@4`, tested in `conformance/effect`) provides rich structured concurrency, fibers, and resource management. In standard Node/Bun runtimes, Effect fibers are multiplexed cooperatively onto a single thread. This issue introduces an Effect v4 companion library that brings true multi-core parallel execution to Effect applications.

## Requirements
1. Create a universal TypeScript library `@kiln/effect`:
   - `spawnActorLayer`: Binds an Actor service to an in-process Tokio Realm, tying its lifecycle to an Effect `Scope`.
   - `KilnWorkerPool.layer`: Implements Effect's `@effect/experimental/WorkerPool` interface using Kiln's in-process Tokio realms and O(1) GC block transfers.
   - `KilnNativeScheduler`: Implements Effect's `Scheduler.Scheduler` interface, dispatching fibers onto Tokio's M:N work-stealing threadpool.
   - `forEachParallel`: Combinator for evaluating collections across worker threads with native fiber cancellation and error short-circuiting.
2. Dual-mode execution:
   - When running compiled under Kiln, bind directly to native host intrinsics (`__kiln_*`).
   - When running in tests under Node.js / Bun / Vitest, fall back gracefully to `node:worker_threads` and `postMessage`.

## Acceptance Criteria
- Code utilizing `@kiln/effect` passes `tsc --strict` without errors.
- Closing an enclosing Effect `Scope` cleanly drains the actor's Tokio mailbox and deallocates the underlying realm.
- Fiber interruption in `forEachParallel` terminates pending execution on other CPU cores via atomic flags.
