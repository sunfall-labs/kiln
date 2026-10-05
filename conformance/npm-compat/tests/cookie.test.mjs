import {
  parseCookie,
  parseSetCookie,
  stringifyCookie,
  stringifySetCookie
} from 'cookie';

console.log('--- parseCookie ---');
console.log(JSON.stringify(parseCookie('foo=bar; baz=qux; theme=dark')));
console.log(JSON.stringify(parseCookie('encoded=hello%20world%21; quoted="quoted-val"')));
console.log(JSON.stringify(parseCookie('spaces =  trimmed  ; flag')));
console.log(JSON.stringify(parseCookie('a=1; a=2')));
console.log(JSON.stringify(parseCookie('')));

console.log('--- stringifyCookie ---');
console.log(stringifyCookie({ foo: 'bar', baz: 'qux' }));
console.log(stringifyCookie({ empty: '', complex: 'hello world!' }));

console.log('--- stringifySetCookie ---');
const expiresDate = new Date('2030-01-01T00:00:00.000Z');
console.log(stringifySetCookie({
  name: 'session_id',
  value: 'xyz-987',
  httpOnly: true,
  secure: true,
  sameSite: 'strict',
  path: '/api',
  domain: 'example.com',
  maxAge: 7200,
  expires: expiresDate,
  partitioned: true
}));

console.log(stringifySetCookie({
  name: 'remember_me',
  value: 'false',
  sameSite: 'lax',
  path: '/'
}));

console.log('--- parseSetCookie ---');
const setCookieHeader = 'session_id=xyz-987; Domain=example.com; Path=/api; Expires=Tue, 01 Jan 2030 00:00:00 GMT; Max-Age=7200; Secure; HttpOnly; SameSite=Strict; Partitioned';
const parsedSet = parseSetCookie(setCookieHeader);
console.log('parsed name:', parsedSet.name);
console.log('parsed value:', parsedSet.value);
console.log('parsed domain:', parsedSet.domain);
console.log('parsed httpOnly:', parsedSet.httpOnly);
console.log('parsed secure:', parsedSet.secure);
console.log('parsed sameSite:', parsedSet.sameSite);
console.log('parsed maxAge:', parsedSet.maxAge);
