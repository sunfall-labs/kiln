// kiln eval smoke test (authored for kiln, not from porffor).
const Complex = require('complex.js');
const z = new Complex(1, 2);
console.log(z.add(new Complex(3, 4)).toString());
console.log(z.mul(new Complex(0, 1)).toString());
console.log(new Complex('3+4i').abs());
console.log(Complex.I.pow(2).toString());
console.log(z.sub(new Complex(1, 1)).toString());
console.log(new Complex(2, 0).sqrt().toString());
