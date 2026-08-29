// Set and Map key on SameValueZero, which differs from `===` in exactly one place: NaN is its
// own key, so a Set can hold one and find it again.
const seen = new Set();
seen.add(1);
seen.add(2);
seen.add(1); // already present
console.log("set", seen.size, seen.has(1), seen.has(99));

// Deleting leaves a hole rather than compacting. Compacting a dense backing store under a live
// iterator shifts everything past the cursor and skips an element -- the exact bug the
// conformance corpus was built to catch.
const items = new Set();
items.add("a");
items.add("b");
items.add("c");
const visited: string[] = [];
for (const item of items) {
  visited.push(item);
  items.delete(item);
}
console.log("deleted while iterating", visited.length, visited.join(","), items.size);

// A Map keeps a re-set key in its original position, which iteration order shows.
const ages = new Map();
ages.set("ana", 1);
ages.set("bo", 2);
ages.set("ana", 3);
const pairs: string[] = [];
for (const entry of ages) {
  pairs.push(entry[0] + "=" + entry[1]);
}
console.log("map", ages.size, ages.get("ana"), pairs.join(","));
console.log("missing key", ages.get("nobody"));

// Keys are matched by identity, so two equal-looking objects are different keys.
const first = { id: 1 };
const second = { id: 1 };
const byObject = new Map();
byObject.set(first, "first");
console.log("identity keys", byObject.get(first), byObject.get(second));
