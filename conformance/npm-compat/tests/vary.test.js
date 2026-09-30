// kiln eval smoke test (authored for kiln, not from porffor).
const vary = require('vary');
function res() {
  return {
    headers: {},
    getHeader(k) { return this.headers[k.toLowerCase()]; },
    setHeader(k, v) { this.headers[k.toLowerCase()] = v; },
  };
}
const r1 = res();
vary(r1, 'Accept');
console.log(r1.headers.vary);
const r2 = res();
vary(r2, ['Accept', 'Accept-Language']);
console.log(r2.headers.vary);
const r3 = res();
vary(r3, '*');
console.log(r3.headers.vary);
const r4 = res();
vary(r4, 'Accept');
vary(r4, 'Origin');
console.log(r4.headers.vary);
