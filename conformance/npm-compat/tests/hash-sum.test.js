// kiln eval smoke test (authored for kiln, not from porffor).
const sum = require('hash-sum');
console.log(sum({a: 1, b: [2, 3]}));
console.log(sum('hello'));
console.log(sum({b: [2, 3], a: 1}));
console.log(sum({a: 1, b: [2, 4]}));
console.log(sum(null));
