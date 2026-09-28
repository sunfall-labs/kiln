import { spawnActor, connectActor, isolate, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [4],
  mailboxCapacity: 1,
});
console.log(await owner.ref.call('increment', [3]));
const address = structuredClone(owner.ref.address);
const second = connectActor(address);
console.log(await second.call('increment', [5]));
try { await second.call('fail', []); }
catch (error) { console.log(error.name, error.remoteName, error.message); }
const data = new ArrayBuffer(4);
new Uint8Array(data)[0] = 23;
const result = await owner.ref.call('echo', [transfer(data)]);
console.log(data.byteLength, new Uint8Array(result)[0]);
const isolated = isolate(() => ({ answer: 42 }));
try { await owner.ref.call('echo', [transfer(isolated)]); }
catch (error) { console.log(error.name, error.message); }
await owner.stop();
try { await second.call('increment', [1]); }
catch (error) { console.log(error.name); }
