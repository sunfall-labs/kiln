// kiln eval smoke test (authored for kiln, not from porffor).
const URL = require('url-parse');

const u = new URL('https://user:pass@host.com:8080/p?q=1#frag');
console.log(u.protocol, u.hostname, u.port);
console.log(u.username, u.password);
console.log(u.pathname, u.query.q, u.hash);

const u2 = new URL('http://example.com');
u2.set('protocol', 'https:');
u2.set('pathname', '/a/b');
u2.set('query', { x: '1' });
console.log(u2.toString());
console.log(URL.qs.parse('a=1&b=2').a);
