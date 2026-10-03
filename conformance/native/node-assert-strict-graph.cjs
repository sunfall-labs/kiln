const assert = require("node:assert");
const { isDeepStrictEqual } = require("node:util");

const key = Symbol("field");
const differentKey = Symbol("field");
const comparisons = [
  [{ [key]: { value: 1 } }, { [key]: { value: 1 } }, true],
  [{ [key]: 1 }, { [key]: 2 }, false],
  [{ [key]: 1 }, {}, false],
  [{ [key]: 1 }, { [differentKey]: 1 }, false],
  [Object.assign([1], { [key]: 1 }), Object.assign([1], { [key]: 2 }), false],
  [Array(1), [undefined], false],
  [new Date(1), new Date(2), false],
  [/a/i, /a/g, false],
  [new Map([[{ id: 1 }, { value: 2 }]]), new Map([[{ id: 1 }, { value: 2 }]]), true],
  [Object.create(null), {}, false],
];
const shared = { value: 1 };
comparisons.push([{ a: shared, b: shared }, { a: { value: 1 }, b: { value: 1 } }, true]);
const left = { [key]: 1 };
const right = { [key]: 1 };
left.self = left;
right.self = right;
comparisons.push([left, right, true]);
const hidden = {};
Object.defineProperty(hidden, key, { value: 1 });
comparisons.push([hidden, {}, true]);

for (let index = 0; index < comparisons.length; index++) {
  const [actual, expected, equal] = comparisons[index];
  assert.strictEqual(isDeepStrictEqual(actual, expected), equal, `comparison ${index}`);
  if (equal) assert.deepStrictEqual(actual, expected);
  else assert.throws(() => assert.deepStrictEqual(actual, expected), { code: "ERR_ASSERTION" });
}
// Legacy equality intentionally ignores symbols.
assert.deepEqual({ [key]: 1 }, { [key]: 2 });
console.log("strict assertion graph comparison passed", comparisons.length);
