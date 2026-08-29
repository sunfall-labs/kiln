// needs setTimeout and an event loop
// expect: slept
import { Effect } from "effect";
Effect.runPromise(Effect.andThen(Effect.sleep("10 millis"), Effect.succeed("slept"))).then((v) => console.log(v));
