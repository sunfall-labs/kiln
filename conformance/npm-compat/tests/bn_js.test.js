// kiln eval smoke test (authored for kiln, not from porffor).
const BN = require('bn.js');

const a = new BN('123456789012345678901234567890');
const b = new BN('987654321098765432109876543210');
console.log('add:', a.add(b).toString(10));
console.log('sub:', b.sub(a).toString(10));
console.log('mul:', new BN('123456789').mul(new BN('987654321')).toString(10));
console.log('pow:', new BN(2).pow(new BN(128)).toString(10));
console.log('hex:', new BN('deadbeef', 16).toString(16));
console.log('divmod:', new BN(100).div(new BN(7)).toString(10), new BN(100).mod(new BN(7)).toString(10));
console.log('neg abs:', new BN(-42).abs().toString(10));
console.log('cmp:', new BN(5).cmp(new BN(9)), new BN(9).cmp(new BN(5)), new BN(7).cmp(new BN(7)));
console.log('isZero:', new BN(0).isZero(), 'isNeg:', new BN(-1).isNeg());
