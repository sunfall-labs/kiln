// kiln eval smoke test (authored for kiln, not from porffor).
const qs = require('querystringify');
console.log(JSON.stringify(qs.parse('?a=1&b=2')));
console.log(JSON.stringify(qs.parse('x=hello%20world')));
console.log(qs.stringify({ a: '1', b: '2' }));
console.log(qs.stringify({ q: 'a b&c' }));
console.log(JSON.stringify(qs.parse('')));
