/**
 * @kiln/effect
 * Effect v4 spellings of `@kiln/concurrency`. Conforms to ADR 0058 (revised 2026-09-06).
 */

import type { Effect, Layer, Scope } from "effect";
import type { Isolated } from "@kiln/concurrency";

export type { Isolated };

/**
 * Spawns an Actor Layer in this realm. Lifecycle is tied to the enclosing Scope: closing the
 * scope calls the instance's `__shutdown`, if it has one.
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
 * Consumes an isolated value. Identity on every engine today.
 */
export function transfer<A>(value: Isolated<A>): A;

/**
 * `Effect.forEach` with unbounded concurrency; `"native"` is a spelling of unbounded.
 * Preserves Effect fiber interruption and error-short-circuiting semantics.
 */
export function forEachParallel<A, B, E, R>(
  elements: Iterable<A>,
  f: (a: A) => Effect.Effect<B, E, R>,
  options?: { readonly concurrency?: "native" | number }
): Effect.Effect<B[], E, R>;
