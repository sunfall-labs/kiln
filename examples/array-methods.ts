// Array methods that take a callback re-enter the interpreter: the built-in pushes a frame for
// the callback and drives the machine until it returns. One shared mechanism, so `map`, `filter`
// and the rest do not each invent a way back in.
const numbers = [1, 2, 3, 4];

console.log("map", numbers.map((n) => n * 2).join(","));
console.log("filter", numbers.filter((n) => n % 2 === 0).join(","));
console.log("find", numbers.find((n) => n > 2));
console.log("some", numbers.some((n) => n > 3), "every", numbers.every((n) => n > 0));
console.log("index argument", numbers.map((n, i) => i).join(","));

let total = 0;
numbers.forEach((n) => {
  total += n;
});
console.log("forEach", total);

// `arguments` is the frame's own, and an arrow sees the enclosing function's -- captured where
// the arrow is written, exactly as `this` is.
function count() {
  return () => arguments.length;
}
console.log("arrow sees the enclosing arguments", count(1, 2, 3)());

function viaMap() {
  return [1, 2].map(() => arguments.length);
}
console.log("through a callback too", viaMap("a", "b", "c").join(","));

// A parameter named `arguments` shadows it, as in JavaScript.
function shadowed(args: number) {
  return args;
}
console.log("shadowing", shadowed(7));
