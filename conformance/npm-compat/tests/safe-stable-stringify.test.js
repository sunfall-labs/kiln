// kiln eval smoke test (authored for kiln, not from porffor).
const stringify = require('safe-stable-stringify');

console.log('sorted keys:', stringify({ b: 1, a: 2, c: 3 }));
console.log('nested:', stringify({ z: { b: 1, a: 2 }, a: [3, 2, 1] }));
const circ = { name: 'x' };
circ.self = circ;
console.log('circular:', stringify(circ));
console.log('bigint:', stringify({ n: 10n }));
console.log('undefined dropped:', stringify({ a: 1, b: undefined }));
console.log('replacer:', stringify({ a: 1, b: 2 }, (k, v) => (k === 'b' ? undefined : v)));
console.log('deterministic:', stringify({ b: 1, a: 2 }) === stringify({ a: 2, b: 1 }));
