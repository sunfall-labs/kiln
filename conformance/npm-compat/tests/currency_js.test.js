// kiln eval smoke test (authored for kiln, not from porffor).
const currency = require('currency.js');
console.log(currency(1234.5, { symbol: '€', separator: '.', decimal: ',' }).format());
console.log(currency(100).add(25).multiply(2).value);
console.log(currency('1,234.56').value);
console.log(currency(10).subtract(3).format());
console.log(currency(99.99, { precision: 0 }).format());
