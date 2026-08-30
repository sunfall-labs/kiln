// The array methods are generic: redirected onto anything array-like, they walk it.
function collect() {
  return Array.prototype.slice.call(arguments);
}
console.log(collect(1, 2, 3).join("-"), collect().length);

// A string walks as its characters.
console.log([].slice.call("abc").join("-"));
console.log(Array.prototype.map.call("abc", (c) => c.toUpperCase()).join(""));
console.log(Array.prototype.indexOf.call("hello", "l"));

// Anything with a `length` walks as its indexed properties.
const like = { 0: "x", 1: "y", length: 2 };
console.log(Array.prototype.join.call(like, "+"));
console.log(Array.prototype.filter.call(like, (v) => v === "y").length);
console.log(Array.prototype.includes.call(like, "x"));

// No `length` means an empty walk, not an error.
console.log(Array.prototype.join.call({ a: 1 }, ","), "|");
console.log(Array.prototype.slice.call(7).length);

// `apply` reaches the same coercion as `call`, and so does a bound one.
console.log(Array.prototype.join.apply("xyz", ["/"]));
const joinOf = Array.prototype.join.bind("xyz");
console.log(joinOf("~"));
