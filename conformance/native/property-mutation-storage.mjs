// Assignment, defineProperty, seal, delete and freeze must agree on one store's descriptors.
const object = { first: 1, second: 2 };
Object.defineProperty(object, 'hidden', {
  value: 3, writable: true, enumerable: false, configurable: true,
});
object.first = 4;
Object.seal(object);
object.second = 5;
console.log('sealed', Object.isSealed(object), Object.keys(object).join(','));
let deleted;
try { deleted = delete object.second; } catch (error) { deleted = error.name; }
console.log('delete sealed', deleted, object.second);
console.log('hidden', Object.getOwnPropertyDescriptor(object, 'hidden').configurable);
Object.freeze(object);
console.log('frozen', Object.isFrozen(object), Object.getOwnPropertyDescriptor(object, 'first').writable);

const fn = function sample() {};
fn.extra = 7;
Object.freeze(fn);
console.log('function', Object.getOwnPropertyDescriptor(fn, 'extra').writable, fn.extra);

const array = [1, 2];
array.extra = 9;
Object.freeze(array);
console.log('array extra', Object.getOwnPropertyDescriptor(array, 'extra').writable, array.extra);
