// kiln eval smoke test (authored for kiln, not from porffor).
const deepEqual = require('deep-equal');
console.log(deepEqual({ a: 1, b: [2] }, { a: 1, b: [2] }));
console.log(deepEqual({ a: 1 }, { a: 2 }));
console.log(deepEqual([1, 2], [1, 2, 3]));
console.log(deepEqual(new Date(0), new Date(0)));
console.log(deepEqual({ a: 1 }, { a: 1, b: undefined }));
