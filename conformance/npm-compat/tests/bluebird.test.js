// kiln eval smoke test (authored for kiln, not from porffor).
const Promise = require('bluebird');

(async () => {
  const v = await Promise.resolve(41).then((x) => x + 1);
  console.log('then:', v);

  const mapped = await Promise.map([1, 2, 3], async (x) => x * 10, { concurrency: 2 });
  console.log('map:', JSON.stringify(mapped));

  const all = await Promise.all([Promise.resolve(1), Promise.resolve(2)]);
  console.log('all:', JSON.stringify(all));

  const filtered = await Promise.filter([1, 2, 3, 4], async (x) => x % 2 === 0);
  console.log('filter:', JSON.stringify(filtered));

  const reduced = await Promise.reduce([1, 2, 3, 4], async (m, x) => m + x, 0);
  console.log('reduce:', reduced);

  const delayed = await Promise.resolve('x').delay(5);
  console.log('delay:', delayed);

  const timed = await Promise.resolve(1).timeout(1000);
  console.log('timeout ok:', timed);

  const spread = await Promise.all([1, 2]).spread((a, b) => a + b);
  console.log('spread:', spread);
})().catch((e) => { console.error('FAILED', e); process.exit(1); });
