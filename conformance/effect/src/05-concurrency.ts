// expect: conc 2,4 | unbounded 2,4,6
import { Effect } from "effect";
Effect.runPromise(Effect.forEach([1, 2], (n) => Effect.succeed(n * 2), { concurrency: 2 })).then((a) => console.log("conc", a.join(",")));
Effect.runPromise(Effect.forEach([1, 2, 3], (n) => Effect.succeed(n * 2), { concurrency: "unbounded" })).then((a) => console.log("unbounded", a.join(",")));
