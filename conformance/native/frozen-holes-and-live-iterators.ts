// Three things that are invisible until something looks: a frozen container, a hole, and a
// collection changed while it is being walked.
//
// Each has a cheap wrong implementation that passes every ordinary test. A frozen array whose
// elements are not properties of a store stays writable. A deleted element that becomes undefined
// reads the same everywhere except `in`. A collection iterator over a snapshot walks the right
// values right up until something deletes one. All three were wrong in the native backend and
// right in the interpreter, which is the only reason this file can check them at all.

// A frozen array refuses an element write and a length change. A module is strict code, so the
// refusal is a TypeError rather than a silence -- the write does not land either way, and which
// of the two happens is the whole of what strictness decides here.
const frozenList = Object.freeze([1, 2]);
function refused(attempt: () => void): string {
  try { attempt(); return "no throw"; } catch (error: any) { return error.constructor.name; }
}
console.log(refused(() => { (frozenList as any)[0] = 9; }), refused(() => { (frozenList as any)[5] = 9; }));
console.log(JSON.stringify(frozenList), frozenList.length, Object.isFrozen(frozenList));
for (const attempt of [() => frozenList.push(3), () => frozenList.pop(),
                       () => frozenList.shift(), () => frozenList.unshift(0),
                       () => frozenList.splice(0, 1)]) {
  try { attempt(); console.log("no throw"); } catch (error: any) { console.log(error.constructor.name); }
}
// The methods that read rather than write still work.
console.log(frozenList.join("-"), frozenList.indexOf(2), frozenList.slice(0, 1).join(""), frozenList.map((x) => x * 2));
// Deleting a non-configurable element is refused the same way, and for the same reason.
console.log(refused(() => { delete (frozenList as any)[0]; }), JSON.stringify(frozenList));

// An object and a function freeze the same way, and nothing else is frozen unless it was.
const frozenObject = Object.freeze({ x: 1 });
console.log(refused(() => { (frozenObject as any).x = 2; }));
console.log(JSON.stringify(frozenObject), Object.isFrozen(frozenObject), refused(() => { delete (frozenObject as any).x; }));
function tagged() {}
Object.freeze(tagged);
console.log(refused(() => { (tagged as any).tag = "x"; }));
console.log((tagged as any).tag, Object.isFrozen(tagged));
console.log(Object.isFrozen([1]), Object.isFrozen({}), Object.isFrozen(1), Object.isFrozen("s"));

// `delete "text".length` names a property that is real and cannot be configured away, so strict
// code raises rather than answering false. Deleting what was never there is still true.
console.log(refused(() => { delete ("text" as any).length; }), delete (5 as any).x, delete ({} as any).nothing);

// A hole is not an undefined, and `reduce` without a seed starts at the first element that is
// *there* -- folding a hole in would turn every sum over a sparse array into NaN.
console.log([, , 5].reduce((total, x) => total + x), [1, , 3].reduce((t, x) => t + x));
console.log([1, , 3].reduce((t, x) => t + x, 10), [, 2, , 4].reduce((t, x) => t + x));
try { [, ,].reduce((t, x) => t + x); } catch (error: any) { console.log("empty", error.constructor.name); }
let seen = 0;
console.log([, , 5].reduce((t, x) => { seen += 1; return t + x; }, 0), seen);

// A collection deleted from mid-walk skips the hole rather than shifting past it.
const shrinking = new Map([["x", 1], ["y", 2], ["z", 3]]);
const visited: string[] = [];
for (const key of shrinking.keys()) {
  visited.push(key);
  if (key === "x") shrinking.delete("y");
}
console.log(visited.join(","), shrinking.size, [...shrinking.keys()].join(","));

// Deleting one already visited does not shift the rest under the cursor.
const behind = new Map([["a", 1], ["b", 2], ["c", 3]]);
const walked: string[] = [];
for (const key of behind.keys()) { walked.push(key); behind.delete(key); }
console.log(walked.join(","), behind.size);

// A key added back after a delete goes to the end: insertion order is the order of first
// appearance *now*, not of the entry that used to be there.
const reused = new Map([["a", 1], ["b", 2]]);
reused.delete("a");
reused.set("a", 3);
console.log([...reused.keys()].join(","), reused.get("a"), reused.size, reused.has("a"));
console.log(reused, new Set([1, 2, 3]));
const emptied = new Set([1, 2]);
emptied.delete(1);
emptied.delete(2);
console.log(emptied.size, emptied, [...emptied].length, emptied.has(1));

// `forEach` and a spread see the same living entries a loop does.
const partial = new Map([["p", 1], ["q", 2], ["r", 3]]);
partial.delete("q");
const collected: string[] = [];
partial.forEach((value, key) => collected.push(key + value));
console.log(collected.join(","), JSON.stringify([...partial]), partial.size);
