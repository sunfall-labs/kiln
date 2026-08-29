// the yield* protocol: needs a user @@iterator to be iterable at all
// expect: gen 2
import { Effect } from "effect";
console.log("gen", Effect.runSync(Effect.gen(function* () { const a = yield* Effect.succeed(1); return a + 1; })));
