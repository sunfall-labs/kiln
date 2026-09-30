// Run with `kiln --one-shot --node run packages/kiln-concurrency/test.js`.
import assert from "node:assert";
import { spawnActor, connectActor, isolate, transfer } from "./index.js";

const owner = await spawnActor({
  module: new URL("./test-counter.js", import.meta.url),
  export: "Counter",
  args: [2],
  mailboxCapacity: 1,
});
let delegated;
try {
  assert.strictEqual(await owner.ref.call("increment", [5]), 7);
  delegated = connectActor(structuredClone(owner.ref.address));
  assert.strictEqual(await delegated.call("increment", [3]), 10);

  const bytes = new ArrayBuffer(4);
  new Uint8Array(bytes)[0] = 23;
  const returned = await delegated.call("echo", [transfer(bytes)]);
  assert.strictEqual(bytes.byteLength, 0);
  assert.strictEqual(new Uint8Array(returned)[0], 23);

  const isolated = transfer(isolate(() => ({ value: 1 })));
  const graph = await delegated.call("echo", [isolated]);
  assert.strictEqual(graph.value, 1);
  await assert.rejects(delegated.call("echo", [isolated]), { name: "DataCloneError" });
} finally {
  await owner.stop();
}
await assert.rejects(delegated.call("increment", [1]), { name: "ActorClosedError" });
console.log("All @kiln/concurrency tests passed!");
