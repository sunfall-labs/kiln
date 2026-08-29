// race and timeout interrupt children by deleting them from a Set while iterating it
// expect: timeout threw TimeoutError | race fast
import { Effect } from "effect";
const slow = Effect.flatMap(Effect.sleep("50 millis"), () => Effect.succeed("slow"));
Effect.runPromise(Effect.timeout(slow, "5 millis"))
  .then((v) => console.log("timeout ok", v))
  .catch((e) => console.log("timeout threw", e && e._tag));
Effect.runPromise(Effect.race(slow, Effect.flatMap(Effect.sleep("2 millis"), () => Effect.succeed("fast")))).then((v) => console.log("race", v));
