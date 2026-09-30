// kiln eval smoke test (authored for kiln, not from porffor).
import setCookieParser from 'set-cookie-parser';

const parsed = setCookieParser.parse(['a=1; Path=/; HttpOnly', 'b=2; Domain=x.com; Secure']);
console.log(JSON.stringify(parsed.map((c) => [c.name, c.value])));
console.log(parsed[0].httpOnly, parsed[0].path, parsed[1].domain, parsed[1].secure);

const single = setCookieParser.parseString('c=3; Max-Age=60; SameSite=Lax');
console.log(single.name, single.value, single.maxAge, single.sameSite);

const split = setCookieParser.splitCookiesString('a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT; b=2');
console.log(split.length);
