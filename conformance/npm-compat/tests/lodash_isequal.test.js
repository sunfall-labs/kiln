// kiln eval smoke test (authored for kiln, not from porffor).
const isEqual = require('lodash.isequal');
console.log(isEqual({ a: [1, 2] }, { a: [1, 2] }));
console.log(isEqual(NaN, NaN));
console.log(isEqual({ a: 1 }, { a: 2 }));
console.log(isEqual([1, { b: 2 }], [1, { b: 2 }]));
console.log(isEqual('a', 'a'));
