const assert = require("node:assert");

// Basic callable and member assertions
assert(true);
assert.ok(true);
assert.equal(1, "1");
assert.notEqual(1, 2);
assert.strictEqual(1, 1);
assert.notStrictEqual(1, "1");
assert.deepEqual({ a: 1, b: "2" }, { a: 1, b: 2 });
assert.deepStrictEqual({ a: 1, b: [2] }, { a: 1, b: [2] });
assert.notDeepStrictEqual({ a: 1 }, { a: "1" });
assert.match("hello world", /world/);
assert.doesNotMatch("hello world", /xyz/);
assert.throws(() => {
  throw new Error("expected error");
});
assert.doesNotThrow(() => {});

// Function identity and names
console.log("assert.name:", assert.name);
console.log("assert.ok === assert:", assert.ok === assert);
console.log("assert.strict.name:", assert.strict.name);
console.log("assert.strict === assert.strict.strict:", assert.strict === assert.strict.strict);

// AssertionError and prototype chain
try {
  assert(false, "custom failure");
} catch (err) {
  console.log("caught:", err instanceof assert.AssertionError);
  console.log("instanceof Error:", err instanceof Error);
  console.log("code:", err.code);
  console.log("operator:", err.operator);
  console.log("message:", err.message);
}

