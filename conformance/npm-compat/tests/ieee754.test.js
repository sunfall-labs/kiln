// kiln eval smoke test (authored for kiln, not from porffor).
const ieee754 = require('ieee754');
const buf = Buffer.alloc(8);
ieee754.write(buf, Math.PI, 0, true, 52, 8);
console.log(ieee754.read(buf, 0, true, 52, 8) === Math.PI);
console.log(buf.toString('hex'));
const b4 = Buffer.alloc(4);
ieee754.write(b4, -1.5, 0, true, 23, 4);
console.log(ieee754.read(b4, 0, true, 23, 4));
console.log(b4.toString('hex'));
const be = Buffer.alloc(8);
ieee754.write(be, 2.5, 0, false, 52, 8);
console.log(ieee754.read(be, 0, false, 52, 8));
