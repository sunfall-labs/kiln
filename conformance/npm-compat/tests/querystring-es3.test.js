// kiln eval smoke test (authored for kiln, not from porffor).
const qs = require('querystring-es3');

console.log(qs.stringify({ a: 1, b: 'x y' }));
console.log(JSON.stringify(qs.parse('a=1&b=x%20y')));
console.log(qs.encode('hello world&more', 'utf8'));
console.log(qs.decode('hello%20world%26more', 'utf8'));
console.log(qs.stringify({ sep: 'a;b' }, ';', ':'));
console.log(JSON.stringify(qs.parse('a:1;b:2', ';', ':')));
