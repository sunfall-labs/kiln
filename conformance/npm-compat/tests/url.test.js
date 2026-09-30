// kiln eval smoke test (authored for kiln, not from porffor).
// NOTE: required by relative path because bare 'url' resolves to the Node builtin.
const url = require('../node_modules/url');

const parsed = url.parse('https://user:pass@example.com:8080/p?q=1#frag', true);
console.log(parsed.protocol, parsed.hostname, parsed.port);
console.log(parsed.auth, parsed.query.q, parsed.hash);

const u = new url.Url('https://example.com/a?x=1');
u.query = Object.assign({}, u.query, { y: '2' });
console.log(u.toString());
console.log(url.format({ protocol: 'https:', host: 'x.com', pathname: '/p' }));
console.log(JSON.stringify(url.resolve('https://a.com/b/c', '../d')));
