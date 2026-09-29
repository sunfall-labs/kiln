// kiln-native: stress=skip
// The 2,000-object snapshot exceeds one nursery chunk. The smaller prepared-transfer probes
// exercise this allocation and handoff under the collector's every-allocation stress mode.
import { prepareTransfer, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});

const source = [];
for (let index = 0; index < 2000; index++) {
  source.push({ id: index, label: 'entry-' + index });
}
const handle = prepareTransfer(source);
source[0].id = -1;
const received = await owner.ref.call('echo', [transfer(handle)]);
if (received.length !== 2000 || received[0].id !== 0 ||
    received[1999].label !== 'entry-1999' || source[0].id !== -1) {
  throw new Error('prepared multichunk graph changed or lost its snapshot');
}
await owner.stop();
console.log('prepared multichunk graph moved');
