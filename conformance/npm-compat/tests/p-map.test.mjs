// kiln eval smoke test (authored for kiln, not from porffor).
import pMap from 'p-map';

const doubled = await pMap([1, 2, 3, 4], async (x) => x * 2);
console.log('map:', JSON.stringify(doubled));

const order = [];
const conc = await pMap([1, 2, 3, 4, 5], async (x) => {
  order.push('start' + x);
  order.push('end' + x);
  return x;
}, { concurrency: 2 });
console.log('concurrency results:', JSON.stringify(conc));

const indexed = await pMap(['a', 'b'], async (v, i) => v + i);
console.log('with index:', JSON.stringify(indexed));

const stopped = await pMap([1, 2, 3], async (x) => {
  if (x === 2) throw new Error('boom');
  return x;
}, { stopOnError: false }).catch((e) => 'caught:' + e.message);
console.log('error path:', JSON.stringify(stopped));

const iterable = await pMap((function* () { yield 10; yield 20; })(), async (x) => x + 1);
console.log('generator input:', JSON.stringify(iterable));
