// Data.TaggedError is `tag => class Base extends Error { _tag = tag }`
// expect: A true MyError x string | B caught:x
import { Effect, Data } from "effect";
class MyError extends Data.TaggedError("MyError")<{ readonly why: string }> {}
const e = new MyError({ why: "x" });
console.log("A", e instanceof Error, e._tag, e.why, typeof e.message);
console.log("B", Effect.runSync(Effect.catchTag(Effect.fail(e), "MyError", (err) => Effect.succeed("caught:" + err.why))));
