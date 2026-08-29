// Destructuring is a tree of patterns, and a default is a wrapper rather than a field on each
// shape -- `[a = 1]`, `{ b = 2 }` and `[[c = 3]]` are one construct at different depths.
const { name, size } = { name: "widget", size: 3 };
console.log("object", name, size);

// A default applies only to `undefined`: not to null, and not to every falsy value.
const { present = "fallback", absent = "used" } = { present: null };
console.log("defaults are for undefined only", present, absent);

const { original: renamed } = { original: "moved" };
console.log("renamed", renamed);

const [first, second, third = 30] = [10, 20];
console.log("array", first, second, third);

const [head, ...tail] = [1, 2, 3, 4];
console.log("rest", head, tail.join(","));

// A hole binds nothing rather than binding undefined.
const [, skipped] = ["ignored", "kept"];
console.log("hole", skipped);

const {
  outer: { inner },
} = { outer: { inner: "nested" } };
console.log("nested", inner);

// Array destructuring goes through the iterator protocol, so anything iterable works -- not
// only arrays. That is why the value is materialised first rather than being indexed directly.
const [x, y] = new Set(["a", "b"]);
console.log("from a set", x, y);

function* pair() {
  yield "left";
  yield "right";
}
const [left, right] = pair();
console.log("from a generator", left, right);
