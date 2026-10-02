// One graph ticket per call, never beside a buffer ticket: the same DataCloneError and message in
// both engines, whichever of them moves the graph. A refused call consumes no ticket.
import { spawnActor, transfer, prepareTransfer, isolate } from "kiln:actors";

const owner = await spawnActor({
  module: new URL("./support/actor-counter.mjs", import.meta.url),
  export: "Counter",
  args: [0],
});
const settle = (promise) => promise.then(() => "accepted", (error) => `${error.name}: ${error.message}`);
const graph = () => transfer(prepareTransfer({ n: 1 }));
const moved = () => transfer(isolate(() => ({ n: 2 })));
const buffer = () => transfer(new ArrayBuffer(4));

console.log("two prepared", await settle(owner.ref.call("echo", [[graph(), graph()]])));
console.log("two isolated", await settle(owner.ref.call("echo", [[moved(), moved()]])));
console.log("buffer then graph", await settle(owner.ref.call("echo", [[buffer(), graph()]])));
console.log("graph then buffer", await settle(owner.ref.call("echo", [[graph(), buffer()]])));
const kept = graph();
console.log("refused keeps ticket", await settle(owner.ref.call("echo", [[kept, buffer()]])),
  await settle(owner.ref.call("echo", [kept])));
await owner.stop();
