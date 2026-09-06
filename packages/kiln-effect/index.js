/**
 * @kiln/effect runtime implementation.
 */

import { Effect, Layer, Scope } from "effect";
import { isolate as baseIsolate, transfer as baseTransfer } from "@kiln/concurrency";

export function spawnActorLayer(service, factory) {
  return Layer.effect(
    service,
    Effect.gen(function* () {
      // 1. Construct the shape inside the factory
      const instance = factory();
      
      // 2. Register finalizer on enclosing Scope to shut down realm
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

export const KilnWorkerPool = {
  layer: Layer.empty,
};

export const KilnNativeScheduler = {
  make(options) {
    return {
      scheduleTask(task) {
        if (typeof globalThis.__kiln_schedule_task === "function") {
          globalThis.__kiln_schedule_task(task);
        } else {
          queueMicrotask(task);
        }
      },
    };
  },
};
