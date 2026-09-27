const o = { a: 1 };
function rb(v) { return v.b; }
let n = 0;
for (let i = 0; i < 100; i++) if (rb(o) === undefined) n++;
Object.prototype.b = 7;
console.log(n, rb(o), rb({ a: 2 }));
class C { constructor() { this.x = 1; } m() { return 1; } }
function rc(v) { return v.zz; }
const c = new C();
for (let i = 0; i < 100; i++) rc(c);
C.prototype.zz = 5;
console.log(rc(c));
