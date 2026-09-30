function Base(a, b) { this.sum = a + b; this.target = new.target; }
const ignored = {};
const Bound = Base.bind(ignored, 10).bind(null, 20);
Bound.prototype = Base.prototype;
class Derived extends Bound { constructor() { super(); this.child = true; } }
const d = new Derived();
console.log(d.sum, d.child, d.target === Derived, d instanceof Derived, d instanceof Base, ignored.sum);
function Returns(x) { return { value: x }; }
const R = Returns.bind(null, 3); R.prototype = Returns.prototype;
class D extends R { constructor() { super(); this.more = 4; } }
const r = new D(); console.log(r.value, r.more);
