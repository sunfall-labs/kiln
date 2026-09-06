// Zero-copy isolated region transfer across boundaries
// expect: QUEUE transfer 42 ok
import { Effect } from "effect";
import { isolate, transfer } from "@kiln/effect";

const data = {
  payload: new Uint8Array([1, 2, 3, 4]),
  count: 42
};

const program = Effect.gen(function* () {
  const isolated = yield* isolate(data);
  const received = transfer(isolated);
  return received.count;
});

Effect.runPromise(program).then((val) => {
  console.log(`QUEUE transfer ${val} ok`);
});
