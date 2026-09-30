// kiln eval smoke test (authored for kiln, not from porffor).
const deepmerge = require('deepmerge');
console.log(JSON.stringify(deepmerge({ a: { b: 1 }, x: 1 }, { a: { c: 2 }, y: 2 })));
console.log(JSON.stringify(deepmerge({ arr: [1, 2] }, { arr: [3] })));
console.log(JSON.stringify(deepmerge({}, { a: 1 })));
console.log(JSON.stringify(deepmerge({ n: null }, { n: 5 })));
