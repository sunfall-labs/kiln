// Arrow functions, function expressions, and object methods are all just functions that happen
// to be written differently. They capture the same way, because they compile the same way.
const double = (x: number) => x * 2;
const add = (a: number, b: number) => a + b;
console.log("concise body", double(21), add(1, 2));

const block = (x: number) => {
  const scaled = x + 1;
  return scaled * 2;
};
console.log("block body", block(4));

// An arrow inside a `return` is a nested function whose free names are captures. Missing that
// case is what made the capture analysis descend unconditionally rather than only when already
// collecting.
function makeAdder(n: number) {
  return (m: number) => n + m;
}
console.log("captured through an arrow", makeAdder(10)(5), makeAdder(100)(5));

const named = function inner(n: number) {
  return n * 3;
};
console.log("function expression", named(7));

// Computed keys are arbitrary expressions, evaluated in source order, and may produce symbols.
const which = "chosen";
const table = {
  [which]: "by name",
  literal: "by literal",
  [Symbol.iterator]: "by symbol",
  describe(prefix: string) {
    return prefix + "!";
  },
};
console.log("computed", table.chosen, table.literal, table[Symbol.iterator]);
console.log("method", table.describe("shorthand"));
