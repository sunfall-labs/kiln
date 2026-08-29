// expect: runSync 1 | pipe 20 | promise 5
import { Effect, pipe } from "effect";
console.log("runSync", Effect.runSync(Effect.succeed(1)));
console.log("pipe", Effect.runSync(pipe(Effect.succeed(1), Effect.map((x) => x + 1), Effect.flatMap((x) => Effect.succeed(x * 10)))));
Effect.runPromise(Effect.succeed(5)).then((v) => console.log("promise", v));
