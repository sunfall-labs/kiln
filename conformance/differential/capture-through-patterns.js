// A destructuring pattern reads names, and a nested function that writes one captures it.
//
// The capture analysis walked a declaration's *initializer* and never its pattern, so a default
// or a computed key inside a nested function -- `const { a = n } = {}` -- did not count as a
// reference to `n`. The enclosing binding was then left unboxed and the nested function could
// not resolve it at all: `n is not defined`, for a name plainly in scope.
function fromDeclaration() {
  const fallback = 1;
  const read = () => { const { a = fallback } = {}; return a; };
  return read();
}
console.log("declaration-default", fromDeclaration());

function fromComputedKey() {
  const key = "k";
  const read = () => { const { [key]: value } = { k: 2 }; return value; };
  return read();
}
console.log("computed-key", fromComputedKey());

function fromArrayPattern() {
  const fallback = 3;
  const read = () => { const [first = fallback] = []; return first; };
  return read();
}
console.log("array-default", fromArrayPattern());

function fromLoopPattern() {
  const fallback = 4;
  const read = () => { for (const { a = fallback } of [{}]) return a; };
  return read();
}
console.log("for-of-default", fromLoopPattern());

function nestedTwice() {
  const fallback = 5;
  return (() => (() => { const { a = fallback } = {}; return a; })())();
}
console.log("two-deep", nestedTwice());

// The pattern's own bound names are still the nested function's, not the enclosing one's.
function shadows() {
  const a = "outer";
  const read = () => { const { a = "inner" } = {}; return a; };
  return `${read()}/${a}`;
}
console.log("shadowing", shadows());
