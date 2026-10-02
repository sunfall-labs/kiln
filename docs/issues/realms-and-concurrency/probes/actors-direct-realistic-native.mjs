// Native-only regression for a graph factory with pointer-bearing temporary arrays.
import { isolate, spawnActor, transfer } from 'kiln:actors';
import { buildGraph, walkGraph } from '../../../bench/concurrency/js/common.mjs';

const expected = walkGraph(buildGraph());
const owner = await spawnActor({
  module: new URL('../../../bench/concurrency/js/actor-graph-target.mjs', import.meta.url),
  export: 'GraphReader',
});

try {
  const handle = isolate(buildGraph);
  const answer = await owner.ref.call('sum', [transfer(handle)]);
  if (answer !== expected) throw new Error('direct graph answer changed');
  console.log('direct realistic graph transferred safely');
} finally {
  await owner.stop();
}
