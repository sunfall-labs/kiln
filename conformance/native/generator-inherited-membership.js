function* values() { yield 1; yield 2; yield 3; }
async function* asyncValues() { yield 4; }
const generator = values();
const asynchronous = asyncValues();
console.log('sync', Symbol.iterator in generator, Symbol.asyncIterator in generator);
console.log('async', Symbol.iterator in asynchronous, Symbol.asyncIterator in asynchronous);
console.log('own', Object.hasOwn(generator, Symbol.iterator), Object.hasOwn(asynchronous, Symbol.asyncIterator));
console.log('values', JSON.stringify(Array.from(generator)));
const prototype = Object.getPrototypeOf(values());
Object.defineProperty(prototype, 'membershipOnly', {
  get() { throw new Error('membership must not call an accessor'); },
  configurable: true
});
console.log('accessor', 'membershipOnly' in values());
