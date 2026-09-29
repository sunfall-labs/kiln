import { spawnActor, prepareTransfer, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});

const source = Array(128).fill(7);
const handle = prepareTransfer(source);
source[127] = 9;
const result = await owner.ref.call('echo', [transfer(handle)]);
if (result.length !== 128 || result[0] !== 7 || result[127] !== 7 || source[127] !== 9) {
  throw new Error('prepared side-backed array did not retain its snapshot');
}

await owner.stop();
console.log('prepared wide array moves once');
