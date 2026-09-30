// kiln eval smoke test (authored for kiln, not from porffor).
const stringify = require('json-stable-stringify');
console.log(stringify({ b: 1, a: 2 }));
console.log(stringify({ z: [3, 2, 1], a: { d: 4, c: 3 } }));
console.log(stringify({ x: 1 }) === '{"x":1}');
console.log(stringify([{ b: 1, a: 2 }]));
console.log(stringify({ b: 2, a: 1 }, { space: 2 }));
