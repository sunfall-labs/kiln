// A method-call site remembers a prototype method by where it lives, not by value. A shape is
// only a key path, so two classes whose constructors store the same keys share one; only the
// prototype chain tells their methods apart. Every way the chain can change is here: the
// receiver's prototype swapped, the method reassigned on its holder, a shadowing method added
// one step down, the method deleted from its holder, and a method two steps up.
class Point { constructor(x, y) { this.x = x; this.y = y; } dist() { return "P" + (this.x + this.y); } }
class Other { constructor(x, y) { this.x = x; this.y = y; } dist() { return "O" + (this.x * this.y); } }
class Base { constructor(v) { this.v = v; } who() { return "base" + this.v; } }
class Mid extends Base {}
class Leaf extends Mid {}
const out = [];
const call = (o) => o.dist();
const who = (o) => o.who();
const p = new Point(3, 4), q = new Point(5, 6), r = new Other(2, 3);
for (let i = 0; i < 3; i++) out.push(call(p), call(r));           // one site, two classes, one shape
Object.setPrototypeOf(q, Other.prototype);
for (let i = 0; i < 3; i++) out.push(call(p), call(q));           // Point's shape, Other's prototype
Point.prototype.dist = function () { return "P2" + this.x; };     // reassigned on the holder
out.push(call(p), call(p));
const leaf = new Leaf(7);
out.push(who(leaf), who(leaf));                                   // holder two steps up
Base.prototype.who = function () { return "base2" + this.v; };
out.push(who(leaf));
Mid.prototype.who = function () { return "mid" + this.v; };       // shadowing one step down
out.push(who(leaf));
delete Mid.prototype.who;
out.push(who(leaf));
delete Base.prototype.who;
out.push(typeof leaf.who);
const deep = Object.create(Object.create(Object.create({ far() { return "far"; } })));
const farCall = (o) => o.far();
out.push(farCall(deep), farCall(deep));                           // three steps: never remembered
console.log(out.join(","));
