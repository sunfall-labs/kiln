// Focused issue-10 numeric-key candidate object probe. Both engines must preserve index order.
import { isolate, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('../../../conformance/native/support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0],
});
try {
  const ticket = transfer(isolate(() => {
    const value = { label: 7 };
    value[127] = 13;
    value[0] = 11;
    return value;
  }));
  const received = await owner.ref.call('inspectNumericAfterAlloc', [ticket]);
  if (received.join(',') !== '0,127,label,11,13,7,4096') {
    throw new Error('numeric-key direct graph changed');
  }
  console.log('numeric-key direct graph moved');
} finally {
  await owner.stop();
}
