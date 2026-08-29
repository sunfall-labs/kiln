// expect: scope released=true
import { Effect } from "effect";
Effect.runPromise(Effect.gen(function* () {
  let released = false;
  yield* Effect.scoped(Effect.gen(function* () {
    yield* Effect.acquireRelease(Effect.succeed("res"), () => Effect.sync(() => { released = true; }));
    return "used";
  }));
  return "released=" + released;
})).then((v) => console.log("scope", v));
