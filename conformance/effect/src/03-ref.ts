// expect: ref 1
import { Effect, Ref } from "effect";
Effect.runPromise(Effect.flatMap(Ref.make(0), (r) => Effect.flatMap(Ref.update(r, (n) => n + 1), () => Ref.get(r)))).then((v) => console.log("ref", v));
