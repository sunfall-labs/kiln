// kiln eval smoke test (authored for kiln, not from porffor).
const { isPlainObject } = require('is-plain-object');

console.log('literal:', isPlainObject({ a: 1 }));
console.log('Object.create(null):', isPlainObject(Object.create(null)));
console.log('new Object:', isPlainObject(new Object()));
console.log('array:', isPlainObject([1, 2]) === false);
console.log('date:', isPlainObject(new Date()) === false);
console.log('class instance:', isPlainObject(new (class Foo {})()) === false);
console.log('null:', isPlainObject(null) === false);
console.log('function:', isPlainObject(() => {}) === false);
console.log('nested:', isPlainObject({ a: { b: [1] } }));
console.log('string:', isPlainObject('x') === false);
