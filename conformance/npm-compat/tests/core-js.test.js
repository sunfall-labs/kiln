// kiln eval smoke test (authored for kiln, not from porffor).
require('core-js/features/array/at');
require('core-js/features/object/has-own');
require('core-js/features/array/flat-map');
console.log([10, 20, 30].at(-1));
console.log(Object.hasOwn({ a: 1 }, 'a'));
console.log(Object.hasOwn({ a: 1 }, 'b'));
console.log([1, 2, 3].flatMap(x => [x, x * 2]).join(','));
console.log('ab'.at(0));
