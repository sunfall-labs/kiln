// Spread goes through the iterator protocol, the same one `for-of` uses, so anything iterable in
// a loop is iterable in a spread. Its length is not known until it runs, which is why the array
// is built incrementally rather than by one sized allocation.
const pair = [1, 2];
console.log("append", [...pair, 3].join(","));
console.log("prepend", [0, ...pair].join(","));
console.log("twice", [...pair, ...pair].join(","));
console.log("between", [0, ...pair, 3, ...pair].join(","));

// A Set is iterable, so it spreads -- and it seeds itself from an iterable too, going through
// `add` so the deduplication is the same code an explicit `add` uses.
const unique = new Set([1, 2, 2, 3]);
console.log("deduplicated", unique.size, [...unique].join(","));

const lookup = new Map([
  ["a", 1],
  ["b", 2],
]);
console.log("map from entries", lookup.size, lookup.get("b"));

// A sequence evaluates every operand and takes the last as its value.
let steps = 0;
const last = ((steps = steps + 1), (steps = steps + 10), steps);
console.log("sequence", last, steps);
