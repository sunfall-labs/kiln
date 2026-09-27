class A { x: number; y: number; constructor(x: number, y: number) { this.x = x; this.y = y; } dist(): number { return this.x + this.y; } }
class B { x: number; y: number; constructor(x: number, y: number) { this.x = x; this.y = y; } dist(): number { return this.x * this.y; } }
function run(o: any): number { return o.dist(); }
let s = 0;
for (let i = 0; i < 5; i++) s += run(new A(2, 3));
console.log(s, run(new B(2, 3)), run(new A(2, 3)), run(new B(4, 5)));
const o: any = { a: 1 };
function rb(v: any) { return v.b; }
console.log(rb(o), rb(o));
(Object.prototype as any).b = 7;
console.log(rb(o), rb(o));
