// kiln eval smoke test (authored for kiln, not from porffor).
const BigNumber = require('bignumber.js');

console.log('0.1+0.2 =', new BigNumber(0.1).plus(0.2).toString());
console.log('mul:', new BigNumber('1.23456789').times('9.87654321').toFixed(8));
console.log('div:', new BigNumber(1).dividedBy(3).toFixed(20));
console.log('pow:', new BigNumber(2).pow(100).toString());
console.log('sqrt:', new BigNumber(2).sqrt().toFixed(15));
console.log('round:', new BigNumber('2.675').toFixed(2), new BigNumber('2.5').toFixed(0, BigNumber.ROUND_HALF_UP));
console.log('cmp:', new BigNumber('1e30').comparedTo('999e27'));
console.log('isInteger:', new BigNumber('42.0').isInteger(), new BigNumber('42.5').isInteger());
console.log('max/min:', BigNumber.max(3, 9, -2).toString(), BigNumber.min(3, 9, -2).toString());
