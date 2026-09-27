// Issue 28, item 3: `F.prototype = undefined` sticks. The native backend used an undefined
// `prototype` slot to mean "not created yet", so after the assignment a read made a fresh object
// and `class U extends F {}` defined a class where node throws. Recorded from node 26.8.1.
function F() {}
F.prototype = undefined;
console.log(F.prototype, "prototype" in F, Object.hasOwn(F, "prototype"));
try { class U extends F {} console.log("no throw"); }
catch (e) { console.log(e.constructor.name); }
// `new` on it links the instance to Object.prototype, as it does for a `null` one.
const o = new F();
console.log(Object.getPrototypeOf(o) === Object.prototype);
function N() {}
N.prototype = null;
console.log(Object.getPrototypeOf(new N()) === Object.prototype);
// It stays undefined across reads, and can be replaced again.
console.log(F.prototype === undefined, F.prototype === undefined);
F.prototype = { tag: "again" };
console.log(F.prototype.tag, new F().tag);
class V extends F {}
console.log(new V().tag);
// A function never assigned still makes its prototype on first read.
function G() {}
console.log(typeof G.prototype, G.prototype.constructor === G);
