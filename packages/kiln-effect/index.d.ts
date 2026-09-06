/**
 * @kiln/effect
 * Effect v4 native multicore concurrency, worker pool, and Tokio scheduler.
 * Conforms to ADR 0057.
 */

import type { Effect, Layer, Scope } from "effect";
import type { Isolated } from "@kiln/concurrency";

export type { Isolated };

/**
 * Spawns an in-process Actor Layer backed by an independent Tokio Realm.
 * Lifecycle is tied to the enclosing Scope: closing the scope shuts down the realm.
 */
export function spawnActorLayer<Id, Shape, Error = never, In = never>(
  service: { readonly key: string; new (...args: any[]): Id },
  factory: () => Shape
): Layer.Layer<Id, Error, In | Scope.Scope>;

/**
 * Marks data as an isolated region within an Effect pipeline.
 */
export function isolate<A>(value: A): Effect.Effect<Isolated<A>>;

/**
 * Prepares an isolated value for O(1) transfer across realm queues.
 */
export function transfer<A>(value: Isolated<A>): A;

/**
 * Evaluates effects across native Tokio worker threads using work-stealing.
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
  export function make(options?: { workers?: "auto" | number }): any;
}
