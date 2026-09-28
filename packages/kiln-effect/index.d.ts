/**
 * @kiln/effect
 * Effect v4 spellings of `@kiln/concurrency`. Conforms to ADR 0058 (revised 2026-09-06).
 */

import type { Context, Effect, Layer, Scope, Scheduler } from "effect";
import type { Isolated, TransferTicket } from "@kiln/concurrency";

export type { Isolated };

/**
 * Spawns an Actor Layer in this realm. Lifecycle is tied to the enclosing Scope: closing the
 * scope calls the instance's `__shutdown`, if it has one.
 */
export function spawnActorLayer<Id, Shape, Error = never, In = never>(
  service:
    | Context.Service<Id, Shape>
    | { readonly key: string; new (...args: any[]): Id }
    | { readonly key: string },
  factory: () => Shape
): Layer.Layer<Id, Error, In | Scope.Scope>;

/**
 * Evaluates a factory and returns an isolated handle within an Effect pipeline.
 */
export function isolate<A>(factory: () => A): Effect.Effect<Isolated<A>>;

/**
 * Creates a one-use transfer ticket. Object graph handoff remains fail-closed until
 * Kiln's runtime can prove sole ownership; ArrayBuffer tickets are supported.
 */
export function transfer<A>(value: Isolated<A> | ArrayBuffer): TransferTicket<A>;

/**
 * `Effect.forEach` with unbounded concurrency; `"native"` is a spelling of unbounded.
 * Preserves Effect fiber interruption and error-short-circuiting semantics.
 */
export function forEachParallel<A, B, E, R>(
  elements: Iterable<A>,
  f: (a: A) => Effect.Effect<B, E, R>,
  options?: { readonly concurrency?: "native" | number }
): Effect.Effect<B[], E, R>;

/**
 * Kiln native worker pool layer for Effect.
 */
export namespace KilnWorkerPool {
  export const layer: Layer.Layer<never, never, never>;
}

/**
 * Kiln native Tokio work-stealing fiber scheduler for Effect.
 */
export namespace KilnNativeScheduler {
  export function make(options?: { workers?: "auto" | number }): Scheduler.Scheduler;
}
