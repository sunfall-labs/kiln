// Destructuring on the left of `=`, where the names already exist.
let a, b, c;
[a, b] = [1, 2];
console.log(a, b);

// The classic swap, which only works because the right side is evaluated first.
[a, b] = [b, a];
console.log(a, b);

// Holes skip a position; a rest takes what is left.
let rest;
[a, , c, ...rest] = [10, 20, 30, 40, 50];
console.log(a, c, rest.join(","));

// Defaults apply to `undefined` alone, not to every falsy value.
[a = 7, b = 8] = [undefined, 0];
console.log(a, b);

// Objects, including renaming and defaults.
let x, y, z;
({ x, y: z } = { x: 1, y: 2 });
console.log(x, z);
({ x = 5, y: z = 6 } = {});
console.log(x, z);

// A rest in an object takes the keys the named ones did not.
let others;
({ x, ...others } = { x: 1, p: 2, q: 3 });
console.log(x, Object.keys(others).join(","));

// Nested, and mixed between the two shapes.
let deep, tail;
({ x: [deep, ...tail] } = { x: [1, 2, 3] });
console.log(deep, tail.join(","));

// The whole thing is an expression: its value is the right-hand side.
let seen;
console.log((seen = [a, b] = [11, 12]) === seen, seen.join(","));

// Assigning through a closure's captured binding.
function make() {
  let inner = 0;
  return [() => inner, (pair) => { [inner] = pair; }];
}
const [read, write] = make();
write([99]);
console.log(read());

// A for-of head is a declaration, but a bare destructuring assignment in a loop is not.
const pairs = [[1, 2], [3, 4]];
let sum = 0;
for (const pair of pairs) {
  [a, b] = pair;
  sum += a + b;
}
console.log(sum);
