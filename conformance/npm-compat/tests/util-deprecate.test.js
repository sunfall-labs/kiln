// kiln eval smoke test (authored for kiln, not from porffor).
const deprecate = require('util-deprecate');
const fn = deprecate((x) => x * 2, 'use something else');
console.log(fn(21));
console.log(deprecate(() => 'ok', 'msg')());
console.log(typeof deprecate);
