const resolve = require.resolve;
const builtin = resolve('node:path');
const local = resolve('./fixtures/require-resolve-target.cjs');
console.log(
  builtin === 'node:path',
  local.endsWith('/fixtures/require-resolve-target.cjs'),
  globalThis.__requireResolveTargetEvaluated === undefined,
);
