let setterCalls = 0;
class IndexedArray extends Array {}
Object.defineProperty(IndexedArray.prototype, '0', {
  set(value) { setterCalls++; },
  configurable: true
});
const first = { value: 17 };
const input = IndexedArray.from([first, 'second']);
const mapped = input.map(value => value);
console.log(JSON.stringify({
  setterCalls,
  firstSame: input[0] === first && mapped[0] === first,
  values: [input[1], mapped[1]],
  keys: [Object.keys(input), Object.keys(mapped)],
  descriptor: Object.getOwnPropertyDescriptor(input, '0')
}));
const controlled = [1, 2];
Object.defineProperty(controlled, '0', { enumerable: false });
Object.defineProperty(controlled, '0', {
  value: 3, writable: true, enumerable: true, configurable: true
});
const closed = Object.preventExtensions([4]);
Object.defineProperty(closed, '0', {
  value: 5, writable: true, enumerable: true, configurable: true
});
let rejected = false;
try {
  Object.defineProperty(Object.freeze([6]), '0', {
    value: 7, writable: true, enumerable: true, configurable: true
  });
} catch (error) {
  rejected = error instanceof TypeError;
}
console.log(JSON.stringify([controlled, Object.keys(controlled), closed, rejected]));
