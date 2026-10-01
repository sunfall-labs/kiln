// kiln eval smoke test (authored for kiln, not from porffor).
// NOTE: require('assert') resolves to the node builtin, so load the npm
// package build explicitly to exercise the published package.
const assert = require('../node_modules/assert/build/assert.js');
assert.strictEqual(1 + 1, 2);
assert.deepStrictEqual({a: [1, 2]}, {a: [1, 2]});
assert.ok(true, 'ok');
assert.throws(() => { throw new Error('boom'); }, /boom/);
assert.doesNotThrow(() => {});
console.log('assert: all passed');
console.log(typeof assert.match, typeof assert.rejects);
