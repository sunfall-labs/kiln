// TypedArray.from gathers iterables before mapping, but reads array-like values while mapping.
const source = { length: 3, 0: 2, 1: 4, 2: 6 };
console.log(Array.from(Uint8Array.from(source, (value, index) => value + index)).join(','));
const mutable = { length: 3, 0: 1, 1: 2, 2: 3 };
console.log(Array.from(Uint8Array.from(mutable, (value, index) => {
  if (index === 0) mutable[1] = 9;
  return value;
})).join(','));
const events: string[] = [];
const iterable = {
  get [Symbol.iterator]() {
    events.push('get iterator');
    return function* () { events.push('yield'); yield 4; yield 5; events.push('done'); };
  },
};
class Bytes extends Uint8Array {
  constructor(length: number) { events.push('construct'); super(length); }
}
const context = { scale: 3 };
const bytes = Bytes.from(iterable, function(this: typeof context, value: number, index: number) {
  events.push('map'); return value * this.scale + index;
}, context);
console.log(bytes instanceof Bytes, Array.from(bytes).join(','), events.join(','));
for (const run of [
  () => Uint8Array.from.call(() => {}, { get length() { throw new Error('length read'); } }),
  () => Uint8Array.from({ length: 1 }, null),
  () => Uint8Array.from(null),
  () => Uint8Array.from({ length: 1n }),
  () => Uint8Array.from({ [Symbol.iterator]: 3 }),
  () => Uint8Array.from.call(function() { return []; }, { length: 1 }),
  () => Uint8Array.from.call(function() { return new Uint8Array(0); }, { length: 1 }),
]) {
  try { run(); console.log('unexpected success'); } catch (error) { console.log(error instanceof TypeError); }
}
console.log(Array.from(BigInt64Array.from({ length: 2, 0: 4n, 1: 5n }, value => value + 1n)).join(','));
console.log(Uint8Array.from.length, Uint8Array.from.name, Object.hasOwn(Uint8Array.from, 'prototype'));
console.log(Object.hasOwn(globalThis, '__kilnIsConstructor'));
console.log(Array.from(Uint8Array.from("12")).join(','));
console.log(Uint8Array.from({ length: -1 }).length);
console.log(Array.from(Uint8Array.from({ [Symbol.iterator]: null, length: 2, 0: 3, 1: 4 })).join(','));
