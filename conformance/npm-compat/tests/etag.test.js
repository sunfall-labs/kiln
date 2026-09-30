// kiln eval smoke test (authored for kiln, not from porffor).
const etag = require('etag');
console.log(etag('hello world'));
console.log(etag('hello world', { weak: true }));
console.log(etag(Buffer.from('abc')));
console.log(etag('hello world') === etag('hello world'));
