/**
 * @kiln/effect runtime implementation.
 *
 * Effect v4 spellings of `@kiln/concurrency`. `forEachParallel` is `Effect.forEach` with
 * unbounded concurrency; `"native"` is accepted as a spelling of that so a program written for
 * a pooled lowering keeps typechecking. The earlier `KilnWorkerPool` and `KilnNativeScheduler`
 * were stubs over an engine hook no engine implemented, and are gone (ADR 0058, revised).
 */

import { Effect, Layer } from "effect";
import { isolate as baseIsolate, transfer as baseTransfer } from "@kiln/concurrency";

export function spawnActorLayer(service, factory) {
  return Layer.effect(
    service,
    Effect.gen(function* () {
      const instance = factory();
      yield* Effect.addFinalizer(() =>
        Effect.sync(() => {
          if (typeof instance.__shutdown === "function") {
            instance.__shutdown();
          }
        })
      );
      return instance;
    })
  );
}

export function isolate(val) {
  return Effect.sync(() => baseIsolate(val));
}

export function transfer(val) {
  return baseTransfer(val);
}

export function forEachParallel(elements, f, options) {
  const conc = options?.concurrency === "native" ? "unbounded" : (options?.concurrency ?? "unbounded");
  return Effect.forEach(elements, f, { concurrency: conc });
}
