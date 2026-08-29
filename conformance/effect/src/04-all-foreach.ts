// Effect.all reaches Array.isArray off a variable holding globalThis.Array
// expect: all 1,2 | seq 2,4,6
import { Effect } from "effect";
Effect.runPromise(Effect.all([Effect.succeed(1), Effect.succeed(2)])).then((a) => console.log("all", a.join(",")));
Effect.runPromise(Effect.forEach([1, 2, 3], (n) => Effect.succeed(n * 2))).then((a) => console.log("seq", a.join(",")));
