// kiln eval smoke test (authored for kiln, not from porffor).
const isNumber = require('is-number');
console.log(isNumber(5));
console.log(isNumber('5'));
console.log(isNumber('foo'));
console.log(isNumber(NaN));
console.log(isNumber(Infinity));
console.log(isNumber('0x11'));
console.log(isNumber(''));
console.log(isNumber(null));
