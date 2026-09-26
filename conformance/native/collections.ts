// A Map and a Set are one structure: a Set is a Map whose values are never read. Insertion
// ordered and scanned linearly, because a hash needs a hash of a *value* and the identity half of
// that is a pointer a collector will one day move -- order is observable and the list is the
// honest structure until then.
const m = new Map<string, number>();
m.set("a", 1).set("b", 2);
console.log(m, m.size, m.get("a"), m.get("z"), m.has("a"), m.has("z"));
console.log([...m], [...m.keys()], [...m.values()], [...m.entries()]);
console.log(m.delete("a"), m.delete("z"), m, m.size);
const s = new Set([1, 2, 2, 3]);
console.log(s, s.size, s.has(2), s.add(4), s.delete(1), s);
console.log([...s], new Map([["x", 1]]), new Set(), new Map());
let out = ""; m.forEach((v: number, k: string) => { out += `${k}=${v},`; }); console.log(out);
for (const [k, v] of new Map([["p", 1]])) { console.log(k, v); }
for (const v of new Set([7])) { console.log(v); }
console.log(new Set([NaN, NaN]).size, new Map([[NaN, 1]]).get(NaN), typeof m, m instanceof Map);

// Keys are compared with SameValueZero, so NaN finds itself where `===` never would, and -0 and
// 0 are the same key.
console.log(new Set([NaN, NaN, 0, -0]).size, new Map([[-0, "a"]]).get(0));

// Re-setting a key keeps its position: iteration reports the order of *first* appearance.
const ordered = new Map([["a", 1], ["b", 2]]);
ordered.set("a", 9);
console.log([...ordered.keys()], ordered.get("a"), ordered.size);

// Deleting shifts rather than swapping, so what remains keeps its order.
const three = new Set(["x", "y", "z"]);
three.delete("y");
console.log([...three], three.size, three.delete("y"));

// `clear` empties it, and the collection is still usable afterwards.
const reused = new Map([["k", 1]]);
reused.clear();
console.log(reused, reused.size, reused.set("k2", 2), reused.size);

// A collection built from anything iterable, including another collection and a generator.
function* letters(): Generator<string> { yield "p"; yield "q"; }
console.log(new Set(letters()), new Set("abc"), new Set([...three]));
console.log(new Map(new Map([["c", 3]])), new Map([...ordered]).size);

// The views are iterators, so each is walked once and is its own iterable.
const walked = new Map([["m", 1], ["n", 2]]);
const view = walked.entries();
console.log([...view], [...view], [...walked.values()]);

// A collection holds references, and two of them are different keys even when they look alike.
const first = { id: 1 };
const second = { id: 1 };
const byIdentity = new Map([[first, "first"]]);
console.log(byIdentity.get(first), byIdentity.get(second), byIdentity.size);

// Mutation during forEach: newly added entries must be visited, deletions skipped.
const mutatedSet = new Set([1]);
const visitedSet: number[] = [];
mutatedSet.forEach((x: number) => {
    visitedSet.push(x);
    if (x < 3) mutatedSet.add(x + 1);
});
console.log(visitedSet.join(","));

const mutatedMap = new Map([["a", 1]]);
const visitedMap: string[] = [];
mutatedMap.forEach((v: number, k: string) => {
    visitedMap.push(`${k}:${v}`);
    if (k === "a") {
        mutatedMap.set("b", 2);
        mutatedMap.set("c", 3);
    }
    if (k === "b") {
        mutatedMap.delete("c");
        mutatedMap.set("d", 4);
    }
});
console.log(visitedMap.join(","));
