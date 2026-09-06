// A method-call site's inline cache is keyed by the receiver's shape. Two objects of one shape
// hold their own values, so a method that is the object's own property must be read from the
// object at the call, not remembered by value from the first object the site saw. On 2026-09-05
// the site remembered the value: every later subscriber got the first one's callback, and a
// promise queue resolved the first item's promise for every item, so `rxjs` and
// `async-concurrency` printed nothing.
class Base { constructor() { this.closed = false; } }
class Sub extends Base {
  constructor(next) { super(); this.dest = { next: next ?? undefined, error: null }; }
  next(v) { this.dest.next(v); }
}
function run(cb) { const s = new Sub(cb); s.next(1); s.next(2); }
run((v) => console.log("a", v));
run((v) => console.log("b", v));
function run2(cb) { const o = { next: cb }; o.next(3); }
run2((v) => console.log("c", v));
run2((v) => console.log("d", v));
function run3(cb) { const s = new Sub(cb); return s; }
run3((v) => console.log("e", v)).next(4);
run3((v) => console.log("f", v)).next(5);
const items = [];
for (let i = 0; i < 3; i++) { const item = {}; new Promise((r) => { item.run = r; }).then((v) => console.log("item", i, v)); items.push(item); }
items.forEach((item, i) => item.run(i * 10));
