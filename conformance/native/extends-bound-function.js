// kiln-native: profile=native-release
//
// Issue 28, item 4: a bound function has no own `prototype`; it inherits the name from its
// [[Prototype]], which is its target's. `class U extends f.bind() {}` is therefore a TypeError
// for an ordinary function, where the native backend refused reading the member. Recorded from
// node 26.8.1.
function f() {}
const b = f.bind(null);
console.log(b.prototype, "prototype" in b, Object.hasOwn(b, "prototype"));
try { class U extends b {} console.log("no throw"); }
catch (e) { console.log(e.constructor.name); }
// Bound twice, and a bound class.
const bb = b.bind(null);
console.log(bb.prototype);
class A { m() { return "A.m"; } }
const bA = A.bind(null);
console.log(bA.prototype);
try { class W extends bA {} console.log("no throw"); }
catch (e) { console.log(e.constructor.name); }
// A bound *derived* class inherits its parent's prototype object through [[Prototype]].
class B extends A {}
const bB = B.bind(null);
console.log(bB.prototype === A.prototype);
try { class X extends bB {} console.log("defines"); }
catch (e) { console.log(e.constructor.name); }
// A `prototype` assigned to the bound function itself is its own and wins.
const own = f.bind(null);
own.prototype = { tag: "own" };
console.log(own.prototype.tag, Object.hasOwn(own, "prototype"));
// A bound built-in has none either.
console.log(Array.bind(null).prototype);
