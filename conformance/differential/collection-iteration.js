// A Map stores pairs; `keys` and `values` project out of them and `entries` hands the pair over.
const m = new Map([["a", 1], ["b", 2]]);
console.log(JSON.stringify([...m.values()]), JSON.stringify([...m.keys()]));
console.log(JSON.stringify([...m.entries()]), JSON.stringify([...m]));

// A Set stores bare values, and its `entries` pairs each with itself.
const s = new Set([1, 2]);
console.log(JSON.stringify([...s.values()]), JSON.stringify([...s.keys()]), JSON.stringify([...s]));
console.log(JSON.stringify([...s.entries()]));

// `Array.from` and destructuring in a for-of head go through the same iterators.
console.log(Array.from(m.values()).join(","), Array.from(m.keys()).join(","));
for (const [key, value] of m) console.log("kv", key, value);

// `forEach` hands (value, key, collection), which is the reverse of what reading it suggests.
m.forEach((value, key) => console.log("map", key, value));
s.forEach((value, key) => console.log("set", key, value));

// Deleting during iteration skips the hole rather than shifting everything after it.
const shrinking = new Map([["x", 1], ["y", 2], ["z", 3]]);
const seen = [];
for (const key of shrinking.keys()) {
  seen.push(key);
  if (key === "x") shrinking.delete("y");
}
console.log(seen.join(","), shrinking.size);

// An iterator is its own iterable, so it can be spread once and is then spent.
const once = m.values();
console.log(JSON.stringify([...once]), JSON.stringify([...once]));
