// kiln eval smoke test (authored for kiln, not from porffor).
const equal = require('fast-deep-equal');
console.log(equal({ a: 1 }, { a: 1 }));
console.log(equal([1, 2], [1, 3]));
console.log(equal(NaN, NaN));
console.log(equal({ a: { b: 2 } }, { a: { b: 2 } }));
console.log(equal('x', 'x'));
console.log(equal({ a: 1, b: 2 }, { a: 1 }));
console.log(equal(new Date(0), new Date(0)));
