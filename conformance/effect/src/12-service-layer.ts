// Context.Service does Object.setPrototypeOf(KeyClass, ServiceProto), and the yieldable
// @@iterator lives on that prototype - so a set prototype must be honoured on a function
// expect: RESULT hi world | 2,4,6
import { Effect, Context, Layer } from "effect";
class Greeter extends Context.Service<Greeter, { greet: (n: string) => string }>()("Greeter") {}
const layer = Layer.succeed(Greeter)({ greet: (n: string) => "hi " + n });
Effect.runPromise(Effect.provide(Effect.gen(function* () {
  const g = yield* Greeter;
  const doubled = yield* Effect.forEach([1, 2, 3], (n) => Effect.succeed(n * 2), { concurrency: "unbounded" });
  yield* Effect.sleep("1 millis");
  return g.greet("world") + " | " + doubled.join(",");
}), layer)).then((v) => console.log("RESULT", v));
