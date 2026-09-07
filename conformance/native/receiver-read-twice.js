// A second read of the same `this` field in a block copies the first only on the shaped row
// path; a getter, a write, or a call between the reads must be observed as the language says.
class V { constructor(x) { this.x = x; this.n = 0; } sq() { return this.x * this.x; } }
class G { constructor() { this.calls = 0; } get x() { this.calls++; return this.calls; } sq() { return this.x * this.x; } }
class W { constructor(x) { this.x = x; } bump() { const a = this.x; this.x = a + 1; return a + this.x; } }
class C { constructor(x) { this.x = x; } twice() { const a = this.x; this.reset(); return a + this.x; } reset() { this.x = 100; } }
class O { constructor() { this.x = { valueOf() { o.x = 7; return 1; } }; } mul() { return this.x * this.x + this.x; } }
const o = new O();
const out = [];
for (let i = 0; i < 3; i++) out.push(new V(i + 2).sq());
const g = new G(); out.push(g.sq(), g.calls);
out.push(new W(5).bump());
out.push(new C(1).twice());
out.push(o.mul());
class D { constructor() { this.x = 3; } f() { return this.x + this.x * this.x - this.x; } }
out.push(new D().f());
console.log(out.join(","));
