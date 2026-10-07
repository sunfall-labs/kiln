// Native receiver-GC control for the realistic graph used by the direct-move probe.
// Ordinary actor messages must keep every graph edge alive through a receiver collection.
import { spawnActor } from 'kiln:actors';
import { buildGraph, walkGraph } from '../../../bench/concurrency/js/common.mjs';

// Full size in ordinary runs; a bounded instance exercises the same nested/shared edges
// when every allocation triggers a whole-heap collection.
function buildStressedGraph() {
  const cats = [];
  for (let i = 0; i < 13; i += 1) cats.push({ code: i * 3, label: 'category-' + i });
  const items = [];
  for (let i = 0; i < 160; i += 1) {
    items.push({
      id: i, name: 'item-' + i, tags: ['t' + (i % 13), 'group-' + (i % 97)],
      price: (i % 1000) / 4, cat: cats[i % 13],
    });
  }
  return items;
}

const graph = process.env.KILN_GC_STRESS ? buildStressedGraph() : buildGraph();
const expected = walkGraph(graph);
const owner = await spawnActor({
  module: new URL('../../../bench/concurrency/js/actor-graph-target.mjs', import.meta.url),
  export: 'GraphReader',
});

try {
  const answer = await owner.ref.call('sum', [graph]);
  if (answer !== expected) throw new Error('cloned graph answer changed');
  console.log('realistic graph cloned safely');
} finally {
  await owner.stop();
}
