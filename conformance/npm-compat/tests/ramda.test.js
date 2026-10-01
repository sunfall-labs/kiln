// kiln eval smoke test (authored for kiln, not from porffor).
const R = require('ramda');

console.log(JSON.stringify(R.map((x) => x * 2, [1, 2, 3])));
console.log(R.pipe(R.add(1), R.multiply(2))(5));
console.log(JSON.stringify(R.filter((x) => x > 2, [1, 2, 3, 4])));
console.log(JSON.stringify(R.reduce((a, b) => a + b, 0, [1, 2, 3])));

const add3 = R.curry((a, b, c) => a + b + c);
console.log(add3(1)(2)(3));
console.log(JSON.stringify(R.uniq([1, 2, 2, 3, 1])));
