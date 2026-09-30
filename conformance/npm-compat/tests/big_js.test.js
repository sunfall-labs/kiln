// kiln eval smoke test (authored for kiln, not from porffor).
const Big = require('big.js');

console.log('0.1+0.2 =', new Big(0.1).plus(0.2).toString());
console.log('sub:', new Big('10').minus('3.3').toString());
console.log('mul:', new Big('1.1').times('1.1').toString());
console.log('div:', new Big(1).div(3).toFixed(10));
console.log('pow:', new Big(2).pow(64).toString());
console.log('round:', new Big('2.5').round(0, 1).toString(), new Big('2.5').round(0, 0).toString());
console.log('cmp:', new Big(5).cmp(9), new Big(9).cmp(5));
console.log('abs/neg:', new Big('-3.14').abs().toString(), new Big('3.14').neg().toString());
console.log('sqrt:', new Big(2).sqrt().toFixed(12));
