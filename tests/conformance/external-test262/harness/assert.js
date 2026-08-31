function assert(actual, message) {
  if (!actual) throw new Error(message || "assertion failed");
}
assert.sameValue = function (actual, expected, message) {
  if (actual !== expected) {
    throw new Error(message || ("expected " + expected + ", got " + actual));
  }
};
