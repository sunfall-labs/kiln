// Native-only positive case for a direct candidate spanning several nursery chunks.
// The small direct-transfer probe covers every-allocation collector stress; that mode
// intentionally invalidates source candidate provenance while this factory is running.
import { isolate, spawnActor, transfer } from 'kiln:actors';

const count = 5000;
const owner = await spawnActor({
  module: new URL('../../../conformance/native/support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});

try {
  const handle = isolate(() => {
    const values = [];
    for (let index = 0; index < count; ++index) values.push({ value: index });
    return values;
  });
  const answer = await owner.ref.call('echo', [transfer(handle)]);
  if (answer.length !== count || answer[0].value !== 0 ||
      answer[count - 1].value !== count - 1 ||
      answer[3141].value !== 3141) {
    throw new Error('direct multichunk transfer changed the graph');
  }
  console.log('direct multichunk move 5000');
} finally {
  await owner.stop();
}
