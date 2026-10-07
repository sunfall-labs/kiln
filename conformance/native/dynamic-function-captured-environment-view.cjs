// Dynamic closures borrow their captured environment. Reads and writes still target the original
// cells, and nested closures capture the same live bindings.
const makeCounter = Function(
  "value",
  "return function() { value = value + 1; return value; };"
);
const counter = makeCounter(3);
const counterFirst = counter();
const counterSecond = counter();
if (counterFirst !== 4 || counterSecond !== 5) throw new Error("captured writes lost their live cell");
console.log("live-cell", counterFirst, counterSecond);

const makeNested = Function(
  "value",
  "return function() { return function() { value = value + 2; return value; }; };"
);
const nested = makeNested(10)();
const nestedFirst = nested();
const nestedSecond = nested();
if (nestedFirst !== 12 || nestedSecond !== 14) throw new Error("nested capture lost its cell");
console.log("nested-cell", nestedFirst, nestedSecond);

const makeConstReader = Function(
  "",
  "const fixed = 7; return function() { fixed = 8; return fixed; };"
);
try {
  makeConstReader()();
} catch (error) {
  if (error.name !== "TypeError") throw error;
  console.log("captured-const", error.name);
}

const makeMutableShadow = Function(
  "",
  "const fixed = 7; return function(fixed) { fixed = fixed + 1; return fixed; };"
);
const mutableShadow = makeMutableShadow();
const shadowFirst = mutableShadow(3);
const shadowSecond = mutableShadow(8);
if (shadowFirst !== 4 || shadowSecond !== 9) throw new Error("mutable local inherited outer restriction");
console.log("mutable-shadow", shadowFirst, shadowSecond);

const deleteWhileTdz = Function(
  "",
  "const remove = function() { return delete later; }; const result = remove(); const later = 1; return result;"
);
console.log("delete-tdz", deleteWhileTdz());
