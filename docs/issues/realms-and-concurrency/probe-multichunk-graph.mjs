// Native-only issue 10 probe: one graph spans several sealed candidate chunks.
import { spawnActor, isolate, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('../../conformance/native/support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0], mailboxCapacity: 1,
});
try {
  const count = 2000;
  const graph = transfer(isolate(() => {
    let link = null;
    for (let index = 0; index < count; index++) link = [index, link];
    return link;
  }));
  const result = await owner.ref.call('sumLinked', [graph]);
  if (result[0] !== count || result[1] !== (count * (count - 1)) / 2)
    throw new Error('multi-chunk graph changed during transfer');
  console.log('multi-chunk graph moved', result[0], result[1]);
} finally {
  await owner.stop();
}
