import { spawnActor, prepareTransfer, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});

const source = { payload: 'q'.repeat(80 * 1024), count: 7 };
const handle = prepareTransfer(source);
source.count = 9;
const received = await owner.ref.call('echo', [transfer(handle)]);
if (received.count !== 7 || source.count !== 9 ||
    received.payload.length !== 80 * 1024 ||
    received.payload[0] !== 'q' || received.payload[80 * 1024 - 1] !== 'q') {
  throw new Error('prepared large string lost its snapshot or contents');
}

await owner.stop();
console.log('prepared large string moves with its graph');
