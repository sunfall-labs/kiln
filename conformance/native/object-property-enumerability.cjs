const assert = require("node:assert");
const enumerable = Function.call.bind(Object.prototype.propertyIsEnumerable);
const key = Symbol("own");
const array = [undefined, , 2];
array[key] = 3;
assert.strictEqual(enumerable(array, 0), true);
assert.strictEqual(enumerable(array, 1), false);
assert.strictEqual(enumerable(array, "length"), false);
assert.strictEqual(enumerable(array, key), true);
let reads = 0;
Object.defineProperty(array, "hidden", { get() { reads++; return 4; }, enumerable: false });
assert.strictEqual(enumerable(array, "hidden"), false);
assert.strictEqual(reads, 0);
Object.freeze(array);
assert.strictEqual(enumerable(array, 2), true);
assert.strictEqual(enumerable("ab", 1), true);
assert.strictEqual(enumerable("ab", "length"), false);
assert.strictEqual(enumerable(1, "toString"), false);
const typed = new Uint8Array([1]);
typed[key] = 4;
assert.strictEqual(enumerable(typed, 0), true);
assert.strictEqual(enumerable(typed, key), true);
let hint;
const coerced = { [Symbol.toPrimitive](value) { hint = value; return key; } };
assert.strictEqual(enumerable(array, coerced), true);
assert.strictEqual(hint, "string");
const marker = {};
assert.throws(() => enumerable(null, { [Symbol.toPrimitive]() { throw marker; } }), (error) => error === marker);
assert.throws(() => enumerable(null, key), TypeError);
let traps = 0;
const proxy = new Proxy({ [key]: 1 }, { getOwnPropertyDescriptor(target, name) {
  traps++;
  assert.strictEqual(name, key);
  return Object.getOwnPropertyDescriptor(target, name);
} });
assert.strictEqual(enumerable(proxy, key), true);
assert.strictEqual(traps, 1);
assert.strictEqual(enumerable(new Proxy({}, { getOwnPropertyDescriptor() {
  return { value: 1, configurable: true };
} }), "field"), false);
assert.strictEqual(enumerable(new Proxy({}, { getOwnPropertyDescriptor() {
  return { value: 1, configurable: true, enumerable: 1 };
} }), "field"), true);
assert.throws(() => enumerable(new Proxy({}, { getOwnPropertyDescriptor() {
  return 1;
} }), "field"), TypeError);
console.log("own property enumerability passed");
