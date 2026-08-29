// forkChild needs module semantics: Effect's scheduler is
// `"setImmediate" in globalThis ? ... : setTimeout` wrapped around its own top-level binding
// expect: fork 7
import { Effect, Fiber } from "effect";
Effect.runPromise(Effect.flatMap(Effect.forkChild(Effect.succeed(7)), (f) => Fiber.join(f))).then((v) => console.log("fork", v));
