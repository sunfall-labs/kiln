// kiln eval smoke test (authored for kiln, not from porffor).
const { createHooks } = require('hookable');

(async () => {
  const hooks = createHooks();
  const order = [];
  hooks.hook('ready', () => order.push('first'));
  hooks.hook('ready', () => order.push('second'));
  await hooks.callHook('ready');
  console.log('order:', JSON.stringify(order));

  const seen = [];
  hooks.hook('data', (x) => seen.push(x * 2));
  await hooks.callHook('data', 21);
  console.log('args:', JSON.stringify(seen));

  let removed = 0;
  const fn = () => removed++;
  const remove = hooks.hook('rm', fn);
  remove();
  await hooks.callHook('rm');
  console.log('removed hook ran:', removed === 0);

  hooks.hookOnce('once', () => order.push('once'));
  await hooks.callHook('once');
  await hooks.callHook('once');
  console.log('hookOnce count:', order.filter((x) => x === 'once').length);

  await hooks.callHook('never-registered');
  console.log('unknown hook ok');
})().catch((e) => { console.error('FAILED', e); process.exit(1); });
