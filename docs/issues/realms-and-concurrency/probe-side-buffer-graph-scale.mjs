// Native-only issue 10 probe: many owned side buffers in one graph certificate.
import { spawnActor, isolate, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('../../conformance/native/support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0], mailboxCapacity: 1,
});
try {
  const graph = transfer(isolate(() => {
    const arrays = [];
    for (let index = 0; index < 1024; index++) arrays.push(Array(128).fill(index & 7));
    return arrays;
  }));
  const received = await owner.ref.call('echo', [graph]);
  if (received.length !== 1024) throw new Error('lost graph members');
  for (let index = 0; index < received.length; index++) {
    if (received[index].length !== 128 || received[index][0] !== (index & 7) ||
        received[index][127] !== (index & 7)) throw new Error('changed graph member');
  }
  console.log('1025 side-backed arrays moved');
} finally {
  await owner.stop();
}
