// kiln eval smoke test (authored for kiln, not from porffor).
const qs = require('qs');

console.log(qs.stringify({ a: 1, b: [1, 2] }));
console.log(JSON.stringify(qs.parse('a=1&b%5B%5D=1&b%5B%5D=2')));
console.log(qs.stringify({ x: { y: 'z' } }, { encode: false }));
console.log(JSON.stringify(qs.parse('name=John+Doe&age=30')));
console.log(qs.stringify({ q: 'a b&c' }));
console.log(JSON.stringify(qs.parse('a[b][c]=deep')));
