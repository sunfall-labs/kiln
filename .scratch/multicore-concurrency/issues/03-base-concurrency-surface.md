# Issue 03: Base TypeScript Concurrency Surface (@kiln/concurrency)

Status: done
Feature: multicore-concurrency
Governing ADR: ADR 0057

Landed 2026-09-06 as the surface both engines actually implement (ADR 0058 revised): `parallelMap<U>(fn): U[]` synchronous, an in-realm async actor proxy, identity `isolate`/`transfer`, the `__kiln_*` hooks removed, `KilnWorkerPool`/`KilnNativeScheduler` removed, and the interpreter given a real `Array.prototype.parallelMap` so the native probe compares the engines without a polyfill. Earlier reality check: packages/kiln-concurrency ships the .d.ts and a Promise.all polyfill; the __kiln_actor_dispatch / __kiln_transfer_region / __kiln_parallel_map hooks it calls are implemented by neither engine, and the Promise<U[]> type contradicts the native lowering, which returns an array synchronously.

## Context
Developers require clean, ergonomic concurrency primitives that do not rely on non-standard syntax or compiler forks. All types and functions must typecheck cleanly under standard, unpatched `tsc --strict`.

## Requirements
1. Implement ambient type declarations and runtime bindings in `crates/semantics/kiln-builtin-surface` under module `@kiln/concurrency`:
   - `Actor`: Base class marking an isolated state machine.
   - `spawnActor<T>(ctor, ...args): ActorRef<T>`: Spawns an actor realm and returns a typed proxy.
   - `ActorRef<T>`: Mapped type wrapping all synchronous and asynchronous methods in `Promise<Awaited<R>>`.
   - `isolate<T>(value: T): Isolated<T>`: Marks an unaliased expression as an isolated region.
   - `transfer<T>(value: Isolated<T>): T`: Prepares an isolated region for O(1) transfer.
2. Extend the global `Array<T>` interface with `parallelMap<U>(fn: (item: T, index: number) => U): Promise<U[]>`.
3. Provide an in-tree runtime implementation backing these functions with Tokio tasks and the native C ABI.

## Acceptance Criteria
- User programs importing `@kiln/concurrency` compile with `tsc --strict --noEmit` with 0 errors and full IDE autocomplete.
- Invoking synchronous actor methods through `ActorRef` correctly produces and resolves native `Promise` instances.
- Negative type checks (e.g. attempting to read an actor method synchronously without `await`) correctly fail at compile time.
