// kiln eval smoke test (authored for kiln, not from porffor).
const gen = require('random-seed');
const r1 = gen.create('seed42');
const r2 = gen.create('seed42');
console.log(r1.intBetween(1, 100), r2.intBetween(1, 100));
console.log(r1.floatBetween(0, 1).toFixed(4));
console.log(r1(100), r1(100));
const r3 = gen.create('other');
console.log(r3.intBetween(1, 100));
console.log(gen.create('seed42').intBetween(1, 100));
