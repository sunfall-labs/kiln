// A call site that cached a prototype method must see every way the chain can change after
// it: assignment, defineProperty, delete, Object.assign, a spread onto the prototype, a computed
// key, a `__proto__` write, setPrototypeOf on the prototype and on the instance, a method added
// on a grandparent, a shadow on the parent, an accessor, and a class re-linked.
class A { m() { return "A"; } }
class B extends A {}
class C extends B {}
const c = new C();
function call() { return c.m(); }
const out = [];
for (let i = 0; i < 3; i++) out.push(call());
B.prototype.m = function () { return "B"; };                 out.push(call());
Object.defineProperty(C.prototype, "m", { value: () => "C", configurable: true, writable: true }); out.push(call());
delete C.prototype.m;                                        out.push(call());
Object.assign(C.prototype, { m() { return "assign"; } });   out.push(call());
delete C.prototype.m;                                        out.push(call());
C.prototype["m"] = () => "computed";                         out.push(call());
delete C.prototype.m;                                        out.push(call());
Object.defineProperty(B.prototype, "m", { get() { return () => "getter"; }, configurable: true }); out.push(call());
delete B.prototype.m;                                        out.push(call());
A.prototype.m = () => "A2";                                  out.push(call());
const other = { m() { return "other"; } };
Object.setPrototypeOf(C.prototype, other);                   out.push(call());
Object.setPrototypeOf(C.prototype, B.prototype);             out.push(call());
Object.setPrototypeOf(c, { m() { return "instance-proto"; } }); out.push(call());
Object.setPrototypeOf(c, C.prototype);                       out.push(call());
c.__proto__ = { m: () => "dunder" };                          out.push(call());
Object.setPrototypeOf(c, C.prototype);                       out.push(call());
C.prototype.__proto__ = { m: () => "proto-dunder" };          out.push(call());
Object.setPrototypeOf(C.prototype, B.prototype);             out.push(call());
const spreadTarget = C.prototype;
Object.assign(spreadTarget, { ...{ m: () => "spread" } });   out.push(call());
delete C.prototype.m;                                        out.push(call());
c.m = () => "own";                                           out.push(call());
delete c.m;                                                  out.push(call());
Object.freeze(B.prototype);                                  out.push(call());
A.prototype.m = () => "A3";                                  out.push(call());
console.log(out.join(","));
// Two shapes, one site, one of them re-linked mid-loop.
class P { v() { return 1; } }
class Q { v() { return 2; } }
const items = [new P(), new Q(), new P(), new Q()];
function sum() { let s = 0; for (const it of items) s += it.v(); return s; }
const sums = [sum(), sum()];
Q.prototype.v = function () { return 20; };
sums.push(sum());
Object.setPrototypeOf(items[0], Q.prototype);
sums.push(sum());
console.log(sums.join(","));
