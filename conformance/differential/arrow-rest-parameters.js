// An arrow has no `arguments` of its own but does have its own rest parameter -- so the two
// cannot come from the same place.
const count = (...args) => args.length;
console.log(count(), count(1), count(1, 2, 3));

const tail = (first, ...rest) => first + ":" + rest.join(",");
console.log(tail(1), tail(1, 2, 3));

// `arguments` inside an arrow is the enclosing function's, even when the arrow has a rest of
// its own that is a different list entirely.
function outer() {
  const inner = (...mine) => mine.length + "/" + arguments.length;
  return inner("a", "b");
}
console.log(outer(1, 2, 3));

// The same holds one level deeper, where the arrow captured through another arrow.
function nested() {
  const middle = () => (...deep) => deep.length + "/" + arguments.length;
  return middle()("x");
}
console.log(nested(9));

// Rest collects nothing when every argument is declared, and a spread feeds it back.
const echo = (...parts) => parts.join("-");
console.log(echo(...[1, 2], 3));
