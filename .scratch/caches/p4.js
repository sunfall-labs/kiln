"use strict";
const out = [];
class A { constructor() { this.x = 1; } m() { return "A.m"; } }
class B extends A { constructor() { super(); this.y = 2; } n() { return "B.n"; } }
class C extends B { k() { return "C.k"; } }
function call(o) { return o.m(); }
function read(o) { return o.m; }
const a = new A(), b = new B(), c = new C();
for (let i = 0; i < 50; i++) { call(a); call(b); call(c); read(b); }
out.push(call(a), call(b), call(c));
A.prototype.m = function () { return "A.m2"; };           // reassigned method
out.push(call(a), call(b), call(c));
B.prototype.m = function () { return "B.m"; };            // shadowing on the way
out.push(call(a), call(b), call(c));
b.m = function () { return "own"; };                      // own shadow (shape change)
out.push(call(b));
const d = new B();
Object.setPrototypeOf(d, { m() { return "swapped"; } });  // swapped prototype
for (let i = 0; i < 5; i++) call(d);
out.push(call(d));
// two classes with one field order
class P { constructor() { this.x = 1; } f() { return "P"; } }
class Q { constructor() { this.x = 2; } f() { return "Q"; } }
function cf(o) { return o.f(); }
for (let i = 0; i < 50; i++) cf(new P());
out.push(cf(new Q()), cf(new P()));
// shaped prototypes via Object.create
const proto = { g: 1 }; const o1 = Object.create(proto); o1.z = 5;
function rg(o) { return o.g; }
for (let i = 0; i < 50; i++) rg(o1);
proto.g = 2; out.push(rg(o1));
const proto2 = { g: 9 }; const o2 = Object.create(proto2); o2.z = 6;
out.push(rg(o2));
// absent
function rh(o) { return o.h; }
for (let i = 0; i < 50; i++) { rh(o1); rh(a); rh({ q: 1 }); rh(Object.create(null)); }
proto.h = "ph"; A.prototype.h = "Ah";
out.push(rh(o1), rh(a), rh(c), rh({ q: 1 }));
Object.prototype.h = "OPh";
out.push(rh({ q: 1 }), rh(Object.create(null)));
delete Object.prototype.h;
// accessor replaces a method
function gm(o) { return o.n(); }
for (let i = 0; i < 50; i++) gm(c);
Object.defineProperty(B.prototype, "n", { get() { return () => "getter"; }, configurable: true });
out.push(gm(c));
// transitioning store under a setter chain
class S { set w(v) { this._w = v * 10; } get w() { return this._w; } }
class T extends S { constructor(v) { super(); this.v = v; } }
function mk(v) { const t = new T(v); t.u = v; return t; }
for (let i = 0; i < 50; i++) mk(i);
Object.defineProperty(S.prototype, "u", { set(v) { this._u = "set:" + v; }, get() { return this._u; }, configurable: true });
out.push(mk(3).u);
// builtin method cache
function mx(a, b) { return Math.max(a, b); }
for (let i = 0; i < 50; i++) mx(1, 2);
Math.max = function () { return "patched"; };
out.push(mx(1, 2));
console.log(out.join(","));
