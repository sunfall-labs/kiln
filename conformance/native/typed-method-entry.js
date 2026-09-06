// A method whose body can be restarted has a typed entry: the receiver arrives matched, the
// slots are read from the receiver-shape cache, every intermediate stays a double, and a
// method call whose cache remembers this method enters it by name with the closure's flags
// decided by the proto. Every way out is a restart through the boxed body: a slot that is not
// a number, a getter, a getter that throws, a receiver whose shape the cache has not seen.
class P { constructor(x, y) { this.x = x; this.y = y; } dist() { return Math.sqrt(this.x * this.x + this.y * this.y); } }
class Q extends P { constructor(x, y, z) { super(x, y); this.z = z; } dist() { return Math.sqrt(this.x * this.x + this.y * this.y + this.z * this.z); } }
class S { constructor(x) { this.x = x; } dist() { return this.x + this.x; } }
class G { get x() { return "g"; } dist() { return this.x * this.x; } }
class T { get x() { throw new Error("boom"); } dist() { return this.x * this.x; } }
const out = [];
const pts = [new P(3, 4), new Q(1, 2, 2), new S("a"), new S(2), new P("3", 4), new G()];
for (let r = 0; r < 3; r++) for (const p of pts) out.push(p.dist());
out.push(String(pts[0].dist() + pts[1].dist()));
try { new T().dist(); } catch (e) { out.push("caught " + e.message); }
// A zero-parameter function called directly has a typed entry too, boxed or double.
function one() { return 1; }
function name() { return "n"; }
let acc = 0; for (let i = 0; i < 5; i++) acc += one();
out.push(acc, name() + name());
// A receiver mutated between calls changes shape; the row for the old shape must not answer.
const m = new P(1, 1); out.push(m.dist()); delete m.x; m.x = 5; out.push(m.dist());
const n = new P(2, 2); n.extra = 1; out.push(n.dist());
// A sloppy-mode method, and a method that reads one name of `this` twice.
const sloppy = { x: 2, dist() { return this.x * this.x + this.x; } };
out.push(sloppy.dist());
console.log(out.join("|"));
