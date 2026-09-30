// kiln eval smoke test (authored for kiln, not from porffor).
const numeral = require('numeral');
console.log(numeral(1234.567).format('0,0.00'));
console.log(numeral(0.5).format('0%'));
console.log(numeral(1000000).format('0.0a'));
console.log(numeral(123).format('00000'));
console.log(numeral(-5000).format('($0,0)'));
console.log(numeral('1,234').value());
