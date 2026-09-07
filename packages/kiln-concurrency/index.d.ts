/**
 * @kiln/concurrency
 * Actors behind an asynchronous proxy, `isolate`/`transfer` identity brands, and a
 * synchronous `parallelMap`. Conforms to ADR 0058 (revised 2026-09-06), which narrows ADR 0057
 * to what both engines implement.
 */

/**
 * Base class marking an Actor.
 * State within an Actor is private to it and reached only through the proxy.
 */
export abstract class Actor {}

/**
 * Transforms all methods of an Actor class into asynchronous Promise-returning calls.
 */
export type ActorRef<T extends Actor> = {
  readonly [K in keyof T as T[K] extends (...args: any[]) => any ? K : never]:
    T[K] extends (...args: infer P) => infer R
      ? (...args: P) => Promise<Awaited<R>>
      : never;
};

/**
 * Constructs the actor and returns a typed asynchronous proxy over it, in this realm.
 */
export function spawnActor<T extends Actor>(
  ctor: new (...args: any[]) => T,
  ...args: ConstructorParameters<new (...args: any[]) => T>
): ActorRef<T>;

/**
 * Phantom nominal brand denoting a disconnected, unaliased object subgraph.
 */
declare const IsolatedBrand: unique symbol;
export type Isolated<T> = T & { readonly [IsolatedBrand]: true };

/**
 * Marks an expression or factory return as an isolated region; `kiln-hir` checks that the
 * value does not escape by another alias.
 */
export function isolate<T>(value: T): Isolated<T>;
export function isolate<T>(factory: () => T): Isolated<T>;

/**
 * Consumes an isolated region. Identity on every engine today.
 */
export function transfer<T>(value: Isolated<T>): T;

declare global {
  interface Array<T> {
    /**
     * `map` that the compiled backend may run on a pool when the callback provably allocates
     * nothing (ADR 0058). Synchronous: the answer is an array, never a promise.
     */
    parallelMap<U>(fn: (item: T, index: number, array: T[]) => U): U[];
  }
}
