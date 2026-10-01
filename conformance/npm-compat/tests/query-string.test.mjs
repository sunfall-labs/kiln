// kiln eval smoke test (authored for kiln, not from porffor).
import queryString from 'query-string';
const { parse, stringify, stringifyUrl, parseUrl } = queryString;

console.log(stringify({ a: 1, b: ['x', 'y'] }));
console.log(JSON.stringify(parse('a=1&b=x&b=y')));
console.log(stringifyUrl({ url: 'https://x.com/path', query: { q: 'hi there' } }));
console.log(JSON.stringify(parseUrl('https://x.com/?a=1#a')));
console.log(stringify({ n: null, u: undefined }, { skipNull: true, strict: false }));
