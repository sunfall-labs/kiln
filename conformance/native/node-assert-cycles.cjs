const assert = require("node:assert");

const actual = { value: 1 };
actual.self = actual;
const expected = { value: 1 };
expected.self = expected;
assert.deepStrictEqual(actual, expected);

const actualArray = [];
actualArray.push(actualArray);
const expectedArray = [];
expectedArray.push(expectedArray);
assert.deepStrictEqual(actualArray, expectedArray);

const other = { value: 2 };
other.self = other;
assert.throws(
  () => assert.deepStrictEqual(actual, other),
  { code: "ERR_ASSERTION" },
);

const twoNodeCycle = {};
const secondNode = {};
twoNodeCycle.self = secondNode;
secondNode.self = twoNodeCycle;
assert.throws(
  () => assert.deepStrictEqual(actual, twoNodeCycle),
  { code: "ERR_ASSERTION" },
);

console.log("node assert cyclic equality passed");
