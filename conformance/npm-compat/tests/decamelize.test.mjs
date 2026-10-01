// kiln eval smoke test (authored for kiln, not from porffor).
import decamelize from 'decamelize';
console.log(decamelize('fooBar'));
console.log(decamelize('fooBar', { separator: '-' }));
console.log(decamelize('FooBarBaz'));
console.log(decamelize('fooBAR', { preserveConsecutiveUppercase: true }));
console.log(decamelize('test'));
