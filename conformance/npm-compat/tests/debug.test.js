// kiln eval smoke test (authored for kiln, not from porffor).
const debug = require('debug');
const d = debug('kiln:test');
console.log(d.namespace);
console.log(d.enabled === false || d.enabled === undefined ? 'off-by-default' : 'on');
debug.enable('kiln:*');
console.log(debug('kiln:test').enabled);
console.log(debug('other:thing').enabled);
debug.disable();
console.log(debug('kiln:test').enabled);
