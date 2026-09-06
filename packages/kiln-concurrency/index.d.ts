/**
 * @kiln/concurrency
 * Native in-process multicore actors, zero-copy region transfer, and parallel data processing.
 * Conforms to ADR 0057.
 */

/**
 * Base class marking an Actor.
 * State within an Actor is strictly private, thread-local, and single-threaded.
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
 * Spawns an actor realm and returns a typed proxy.
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
 * Marks an expression or factory return as an isolated region.
 * In Kiln, triggers static escape analysis.
 */
export function isolate<T>(value: T): Isolated<T>;
export function isolate<T>(factory: () => T): Isolated<T>;

/**
 * Consumes an isolated region and prepares it for O(1) cross-realm transfer.
 */
export function transfer<T>(value: Isolated<T>): T;

declare global {
  interface Array<T> {
    /**
     * Executes a pure parallel map across worker threads.
     */
    parallelMap<U>(fn: (item: T, index: number) => U): Promise<U[]>;
  }
}
