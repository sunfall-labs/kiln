// kiln eval smoke test (authored for kiln, not from porffor).
const stringify = require('fast-json-stable-stringify');
console.log(stringify({ b: 1, a: { d: 4, c: 3 } }));
console.log(stringify({ z: [3, 2, 1], a: 'x' }));
console.log(stringify([{ b: 2, a: 1 }]));
