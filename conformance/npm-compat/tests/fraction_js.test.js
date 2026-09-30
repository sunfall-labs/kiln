// kiln eval smoke test (authored for kiln, not from porffor).
const Fraction = require('fraction.js');
console.log(new Fraction('1/3').add(new Fraction('1/6')).toFraction());
console.log(new Fraction(0.75).toString());
console.log(new Fraction(2, 3).mul(9).valueOf());
console.log(new Fraction(10).div(new Fraction(4)).toFraction());
console.log(new Fraction(3, 9).toFraction());
console.log(new Fraction(-7, 2).abs().toFraction());
