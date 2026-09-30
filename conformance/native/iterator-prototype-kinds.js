const arrayIterator = [1][Symbol.iterator]();
const stringIterator = "x"[Symbol.iterator]();
const regexpStringIterator = "x".matchAll(/x/g);
const typedArrayIterator = new Uint8Array([1]).values();

console.log(Object.prototype.toString.call(arrayIterator));
console.log(Object.prototype.toString.call(stringIterator));
console.log(Object.prototype.toString.call(regexpStringIterator));
console.log(Object.prototype.toString.call(typedArrayIterator));
console.log(Object.getPrototypeOf(arrayIterator) !== Object.getPrototypeOf(stringIterator));
console.log(Object.getPrototypeOf(arrayIterator).next === arrayIterator.next);
console.log(Object.hasOwn(arrayIterator, "next"));
console.log(arrayIterator.next().value, stringIterator.next().value);
