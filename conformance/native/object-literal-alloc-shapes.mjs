// Object literal allocation: every literal shape must keep its semantics while the site's
// remembered shape, plain-object finalization skip and string-literal fast load are in play.
// Prints one line per case; node is the oracle.
function one(i) { return { a: i }; }
function three(i) { return { a: i, b: "x", c: true }; }
function eight(i) { return { a: i, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8 }; }
function nested(i) { return { status: i, headers: { "content-type": "text/plain" } }; }

let count = 0;
const kept = [];
for (let i = 0; i < 60000; i++) {
  count += one(i).a + three(i).a + eight(i).h + nested(i).status;
  if (i % 997 === 0) kept.push(three(i), nested(i));
}
console.log("count", count);
console.log("kept", kept.length, JSON.stringify(kept[0]), JSON.stringify(kept[kept.length - 1]));

// identity: each evaluation is a fresh object
const same = () => ({ a: 1 });
console.log("identity", same() === same(), (() => { const o = same(); return o === o; })());

// escape via closure capture and store
function capture(i) { const o = { v: i, s: "str" + i }; return () => o; }
const fns = [];
for (let i = 0; i < 20000; i++) { const f = capture(i); if (i % 1000 === 0) fns.push(f); }
console.log("closure", fns.map((f) => f().v + f().s).join(","));

// getters / setters / computed / __proto__ / spread / numeric-like key ordering
let log = [];
const k = "dyn";
const g = { get a() { log.push("get"); return 1; }, set a(v) { log.push("set" + v); }, [k]: 2, b: 3 };
g.a = 5; void g.a;
console.log("accessors", log.join(","), Object.keys(g).join(","), g.dyn);
const p = { __proto__: { inherited: 1 }, own: 2 };
console.log("proto", p.inherited, Object.keys(p).join(","), Object.getPrototypeOf(p).inherited);
const sp = { ...{ x: 1, y: 2 }, y: 3, z: 4 };
console.log("spread", JSON.stringify(sp));
const nk = { b: 1, 2: "two", a: 2, 1: "one", "-1": "neg", "01": "lead" };
console.log("order", Object.keys(nk).join(","));
const dup = { a: 1, a: 2, b: 3 };
console.log("dup", JSON.stringify(dup));

// a literal site that sees a double and then a non-number in the same slot
function mixed(v) { return { a: v, b: 1 }; }
console.log("mixed", [1.5, "s", 2, null, undefined, {}, 3.25].map((v) => typeof mixed(v).a).join(","));

// allocate enough to force several collections while holding survivors
const held = [];
for (let i = 0; i < 200000; i++) {
  const o = nested(i);
  if (i % 5000 === 0) held.push(o);
}
console.log("held", held.length, held.every((o, j) => o.status === j * 5000 && o.headers["content-type"] === "text/plain"));

// objects that grow side storage still die correctly
let big = 0;
for (let i = 0; i < 20000; i++) {
  const o = one(i);
  for (let j = 0; j < 12; j++) o["k" + j] = j;
  o[3] = i;
  big += Object.keys(o).length;
}
console.log("grown", big);

// Leaf literal reads (`f(x).k` where f only returns a literal): the call and the object may
// be elided, so nothing observable may depend on either.
function pickA(i) { return { a: i, b: 2, c: "s" }; }
function pickLate(i, j) { return { first: i, second: j }; }
let leafTotal = 0;
for (let i = 0; i < 30000; i++) leafTotal += pickA(i).a + pickA(i).b + pickLate(i, 1).second;
console.log("leaf", leafTotal, pickA(5).c, pickLate(1).second, pickLate(1, 2, 3).first);
// a key the literal does not define reads through the prototype chain
Object.prototype.inherited = "from-proto";
console.log("leaf-proto", pickA(1).inherited, pickA(1).missing, pickA(1).toString === Object.prototype.toString);
delete Object.prototype.inherited;
// the call may not be elided when the result is used twice or escapes
const r = pickA(9);
console.log("leaf-escape", r.a, r.b, r === r, pickA(9) === pickA(9), Object.keys(r).join(","));
// arguments are still evaluated, in order, even when unused by the read
const order = [];
function note(x) { order.push(x); return x; }
console.log("leaf-order", pickLate(note(1), note(2)).first, order.join(","));
// nested reads
console.log("leaf-nested", pickA(pickA(7).a).a);
// a callee that gets reassigned is not the leaf any more
let swap = (i) => ({ a: "other" + i });
function swapped(i) { return { a: i }; }
const results = [];
for (let n = 0; n < 3; n++) { results.push(swapped(n).a); if (n === 0) swapped = swap; }
console.log("leaf-reassign", results.join(","));
