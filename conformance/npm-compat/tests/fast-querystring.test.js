// kiln eval smoke test (authored for kiln, not from porffor).
const qs = require('fast-querystring');
console.log(JSON.stringify(qs.parse('a=1&b=2&a=3')));
console.log(qs.stringify({ a: 1, b: 'x y' }));
console.log(JSON.stringify(qs.parse('flag')));
console.log(qs.stringify({ q: 'a&b=c' }));
