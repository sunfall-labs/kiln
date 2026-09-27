class A { constructor(x) { this.x = x; } m(s) { return this.x; } }
class B extends A { constructor(x) { super(x); this.y = 1; } }
const a = new A(1), b = new B(2);
function run(n) { let t = 0; for (let i = 0; i < n; i++) { t += a.m("s"); t += b.m("s"); } return t; }
const t0 = Date.now(); const r = run(5000000); console.log(r, Date.now() - t0, "ms");
