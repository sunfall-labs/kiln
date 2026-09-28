// kiln-native: parallelism=1,2
import { spawnActor } from 'kiln:actors';

const owners = await Promise.all(Array.from({ length: 24 }, (_, index) => spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [index],
})));
const results = await Promise.all(owners.map((owner) => owner.ref.call('increment', [1])));
if (results.some((value, index) => value !== index + 1)) throw new Error('actor reply mismatch');
const resumed = await Promise.all(owners.map((owner) => owner.ref.call('waitTurn', [])));
if (resumed.some((value, index) => value !== index + 1)) throw new Error('actor wake mismatch');
const refusal = await owners[0].ref.call('waitRejected', []);
if (refusal !== 'TypeError: Atomics.wait cannot block an actor; use actor messages') {
  throw new Error(`actor wait policy: ${refusal}`);
}
await Promise.all(owners.map((owner) => owner.stop()));
console.log('actors on shared lanes', results.length, refusal);
