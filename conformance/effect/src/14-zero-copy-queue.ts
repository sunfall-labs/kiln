// An isolated graph produces an opaque ticket; graph handoff remains fail-closed until
// the runtime proves sole ownership at a safepoint.
// expect: QUEUE DataCloneError ok
import { Effect } from "effect";
import { isolate, transfer } from "@kiln/effect";

const data = {
  payload: new Uint8Array([1, 2, 3, 4]),
  count: 42
};

const program = Effect.gen(function* () {
  const isolated = yield* isolate(() => data);
  const ticket = transfer(isolated);
  try {
    structuredClone(ticket);
    return "unexpected clone";
  } catch (error) {
    return (error as Error).name;
  }
});

Effect.runPromise(program).then((val) => {
  console.log(`QUEUE ${val} ok`);
});
