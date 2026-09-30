// kiln eval smoke test (authored for kiln, not from porffor).
import { parseCookie, stringifyCookie, stringifySetCookie } from 'cookie';
console.log(JSON.stringify(parseCookie('foo=bar; baz=qux')));
console.log(stringifyCookie({ foo: 'bar' }));
console.log(stringifySetCookie({ name: 'session', value: 'abc123', httpOnly: true, maxAge: 3600, path: '/' }));
console.log(JSON.stringify(parseCookie('a=1; a=2')));
console.log(stringifyCookie({ empty: '' }));
