// kiln eval smoke test (authored for kiln, not from porffor).
const {__assign, __rest, __spreadArray, __read} = require('tslib');
console.log(JSON.stringify(__assign({a: 1}, {b: 2}, {a: 3})));
console.log(JSON.stringify(__rest({a: 1, b: 2, c: 3}, ['a'])));
console.log(JSON.stringify(__spreadArray([1, 2], [3, 4])));
console.log(JSON.stringify(__read([10, 20, 30], 2)));
