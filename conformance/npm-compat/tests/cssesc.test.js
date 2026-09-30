// kiln eval smoke test (authored for kiln, not from porffor).
const cssesc = require('cssesc');
console.log(cssesc('a>b #c.d', { isIdentifier: false }));
console.log(cssesc('0abc', { isIdentifier: true }));
console.log(cssesc('hello world'));
console.log(cssesc('--custom', { isIdentifier: true }));
