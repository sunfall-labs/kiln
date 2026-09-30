// kiln eval smoke test (authored for kiln, not from porffor).
const Decimal = require('decimal.js');
console.log(new Decimal(0.1).plus(0.2).toString());
console.log(new Decimal(2).pow(10).toString());
console.log(new Decimal('1.005').toDP(2).toString());
console.log(Decimal.sqrt(2).toDP(5).toString());
console.log(new Decimal(10).dividedBy(3).toDP(4).toString());
