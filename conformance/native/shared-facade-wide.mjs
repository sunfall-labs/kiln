// A wide sealed graph reuses every live child identity across table growth and collection.
import { share, isShared } from 'kiln:shared';

const source = Array.from({ length: 257 }, (_, index) => ({ index }));
source.push(source[17]);
const graph = share(source);
const kept = [];
for (let index = 0; index < 257; index++) {
  const child = graph[index];
  if (!isShared(child) || child.index !== index) throw new Error('shared child changed');
  if (index % 7 === 0) kept.push([index, child]);
}
for (let pass = 0; pass < 4; pass++) {
  for (let index = 256; index >= 0; index--) {
    if (graph[index].index !== index) throw new Error('shared child read changed');
  }
  for (const [index, child] of kept) {
    if (graph[index] !== child) throw new Error('shared child identity changed');
  }
  if (graph[257] !== graph[17]) throw new Error('shared alias changed');
}
console.log('wide shared identities stable');
