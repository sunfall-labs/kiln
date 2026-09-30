// kiln eval smoke test (authored for kiln, not from porffor).
const stringify = require('fast-safe-stringify');

console.log('basic:', stringify({ b: 1, a: 2 }));
const circ = { x: 1 };
circ.me = circ;
console.log('circular:', stringify(circ));
console.log('decycled:', stringify(circ, undefined, undefined, (key, value) => (key === 'me' ? '[skip]' : value)));
const arr = [1, 2];
arr.push(arr);
console.log('array circular:', stringify(arr));
console.log('replacer:', stringify({ a: 1, secret: 2 }, (k, v) => (k === 'secret' ? '***' : v)));
console.log('stable stringify fn:', typeof stringify.stable === 'function');
console.log('stable sorted:', stringify.stable({ b: 1, a: 2 }));
