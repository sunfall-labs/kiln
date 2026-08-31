// `for...in` is the one enumeration in the language that is not about own properties: it walks the
// prototype chain. That single difference from `Object.keys` is where the interesting cases are --
// a name found twice is visited once, a name hidden nearer the object is not visited at all, and
// what counts as "hidden" is *any* own property rather than an enumerable one.

const base: any = { inherited: 1, shadowed: 2 };
const derived: any = Object.create(base);
derived.own = 3;
derived.shadowed = 4;
const walked: string[] = [];
for (const key in derived) { walked.push(key); }
// Own names first, in insertion order, then the chain's -- and `shadowed` appears once, with the
// nearer value, because it was already visited.
console.log(walked, derived.shadowed, Object.keys(derived));

// Shadowing is by *name*, not by enumerability: a non-enumerable own property hides an enumerable
// one further out rather than merely losing its own turn. It is the case a union of each level's
// enumerable keys gets wrong, and the reason the walk has to remember every name it has passed.
const hiding: any = Object.create(base);
Object.defineProperty(hiding, "shadowed", { value: 9, enumerable: false });
const past: string[] = [];
for (const key in hiding) { past.push(key); }
console.log(past, hiding.shadowed, Object.keys(hiding));

// A class's methods live on the prototype and are *not* enumerated, so an instance shows the
// fields its constructor assigned and nothing the class declared -- including the `constructor`
// link the prototype carries back.
class Point { x = 1; y = 2; describe(): string { return `${this.x},${this.y}`; } }
const seen: string[] = [];
for (const key in new Point()) { seen.push(key); }
console.log(seen, Object.keys(new Point()), "describe" in new Point());

// Three deep, with a name at every level.
const top: any = { level: "top", onlyTop: 1 };
const middle: any = Object.create(top);
middle.level = "middle";
middle.onlyMiddle = 2;
const bottom: any = Object.create(middle);
bottom.level = "bottom";
const depth: string[] = [];
for (const key in bottom) { depth.push(key); }
console.log(depth, bottom.level);

// An array enumerates its indices as strings, and then the names it carries beside them: an array
// is an object with a length, and `a.total = 1` is an ordinary own property of it.
const totals: any = [10, 20];
totals.total = 30;
const indices: string[] = [];
for (const key in totals) { indices.push(key); }
console.log(indices, "total" in totals, "0" in totals, "5" in totals, totals.length);

// An object with no prototype at all has nothing to walk, and `Object.create(null)` is the way a
// program asks for one -- which is why it is the idiom for a dictionary that cannot inherit a key.
const dictionary: any = Object.create(null);
dictionary.key = "value";
const bare: string[] = [];
for (const key in dictionary) { bare.push(key); }
console.log(bare, Object.getPrototypeOf(dictionary), dictionary.key);
// What is deliberately *not* asked here: `"toString" in Object.create(null)`. The native backend
// answers false, which is right, and the interpreter answers true -- a bug on the interpreter's
// side of a chain that ends in an explicit null. This corpus compares the two engines and the
// interpreter is its oracle, so recording the disagreement here would record the wrong answer.

// The key list is a snapshot taken before the first step, but a name the body deletes is not
// visited afterwards: the loop re-asks whether each key is still there.
const vanishing: any = { a: 1, b: 2, c: 3 };
const survived: string[] = [];
for (const key in vanishing) {
  survived.push(key);
  delete vanishing.c;
}
console.log(survived, vanishing);

// Nothing at all is a legal walk: neither a name nor an error.
let nothing = 0;
for (const key in {}) { nothing += 1; }
for (const key in Object.create(base)) { nothing += 100; }
console.log(nothing);
