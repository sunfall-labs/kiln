// kiln eval smoke test (authored for kiln, not from porffor).
const { transform } = require('sucrase');

console.log(transform('const x: number = 1;', { transforms: ['typescript'] }).code);
console.log(
  transform('import a from "a"; export default a;', { transforms: ['imports'] }).code
);
console.log(
  transform('enum E { A, B }', { transforms: ['typescript'] }).code.trim().split('\n')[0]
);
console.log(transform('let {a, b} = c;', { transforms: [] }).code.trim());
