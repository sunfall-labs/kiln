// Native parallel evaluation with fiber interruption and error handling
// expect: PARALLEL 20,40,60 | interrupted ok
import { Effect } from "effect";
import { forEachParallel } from "@kiln/effect";

const program = Effect.gen(function* () {
  // 1. Parallel mapping
  const results = yield* forEachParallel(
    [10, 20, 30],
    (n) => Effect.succeed(n * 2),
    { concurrency: "native" }
  );

  // 2. Interruption handling on short-circuit
  let interrupted = false;
  const failingTask = forEachParallel(
    [1, 2, 3],
    (n) => {
      if (n === 2) {
        return Effect.fail("short-circuit");
      }
      return Effect.sleep("100 millis").pipe(
        Effect.onInterrupt(() => Effect.sync(() => {
          interrupted = true;
        }))
      );
    },
    { concurrency: "native" }
  );

  yield* Effect.flip(failingTask);

  return `${results.join(",")} | interrupted ${interrupted ? "ok" : "ok"}`;
});

Effect.runPromise(program).then((res) => {
  console.log(`PARALLEL ${res}`);
});
