// kiln eval smoke test (authored for kiln, not from porffor).
const { SyncHook, SyncBailHook, AsyncSeriesHook } = require('tapable');

const hook = new SyncHook(['a', 'b']);
const seen = [];
hook.tap('first', (a, b) => seen.push(['first', a + b]));
hook.tap({ name: 'second', stage: -10 }, (a, b) => seen.push(['second', a * b]));
hook.call(3, 4);
console.log(JSON.stringify(seen));

const bail = new SyncBailHook(['x']);
bail.tap('t1', () => undefined);
bail.tap('t2', (x) => `bailed:${x}`);
console.log(bail.call('arg'));

(async () => {
  const asyncHook = new AsyncSeriesHook(['v']);
  const order = [];
  asyncHook.tapPromise('p1', async (v) => order.push(v * 2));
  asyncHook.tapPromise('p2', async (v) => order.push(v * 3));
  await asyncHook.promise(5);
  console.log(JSON.stringify(order));
})();
