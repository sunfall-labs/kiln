// Dynamic closures borrow their captured environment. Reads and writes still target the original
// cells, and nested closures capture the same live bindings.
const makeCounter = Function(
  "value",
  "return function() { value = value + 1; return value; };"
);
const counter = makeCounter(3);
console.log("live-cell", counter(), counter());

const makeNested = Function(
  "value",
  "return function() { return function() { value = value + 2; return value; }; };"
);
const nested = makeNested(10)()();
console.log("nested-cell", nested(), nested());

const makeConstReader = Function(
  "",
  "const fixed = 7; return function() { fixed = 8; return fixed; };"
);
try {
  makeConstReader()();
} catch (error) {
  console.log("captured-const", error.name);
}
