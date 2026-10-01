// kiln eval smoke test (authored for kiln, not from porffor).
const JSBI = require('jsbi');
console.log(JSBI.toNumber(JSBI.add(JSBI.BigInt(1), JSBI.BigInt(2))));
console.log(JSBI.BigInt('9007199254740993').toString());
console.log(JSBI.multiply(JSBI.BigInt(6), JSBI.BigInt(7)).toString());
console.log(JSBI.lessThan(JSBI.BigInt(3), JSBI.BigInt(4)));
console.log(JSBI.subtract(JSBI.BigInt(10), JSBI.BigInt(4)).toString());
console.log(JSBI.exponentiate(JSBI.BigInt(2), JSBI.BigInt(10)).toString());
