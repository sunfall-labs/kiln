// kiln eval smoke test (authored for kiln, not from porffor).
const _ = require('lodash');
console.log(JSON.stringify(_.chunk([1, 2, 3, 4, 5], 2)));
console.log(JSON.stringify(_.uniq([1, 1, 2, 3, 3])));
console.log(JSON.stringify(_.groupBy(['one', 'two', 'three'], 'length')));
console.log(JSON.stringify(_.merge({a: {b: 1}}, {a: {c: 2}})));
console.log(_.capitalize('hello world'));
console.log(JSON.stringify(_.flattenDeep([1, [2, [3]]])));
console.log(_.VERSION);
