// kiln eval smoke test (authored for kiln, not from porffor).
const Hashids = require('hashids');
const h = new Hashids('salty', 8);
const enc = h.encode(1, 2, 3);
console.log(enc, JSON.stringify(h.decode(enc)));
console.log(h.encode(0), h.encodeHex('deadbeef'));
const h2 = new Hashids();
console.log(h2.encode(42).length > 0);
console.log(JSON.stringify(h.decode('zzzzzzzz')));
