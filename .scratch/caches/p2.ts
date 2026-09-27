class A { x: number; y: number; constructor(x: number, y: number) { this.x = x; this.y = y; } f(k: number): number { return this.x + this.y + k; } }
class B { x: number; y: number; constructor(x: number, y: number) { this.x = x; this.y = y; } f(k: number): number { return this.x * this.y * k; } }
function run(o: A, k: number): number { return o.f(k); }
let s = 0;
const a = new A(2, 3);
for (let i = 0; i < 1000; i++) s += run(a, 1);
const b: any = new B(2, 3);
console.log(s, run(b, 2), run(a, 2), run(new B(4, 5) as any, 1));
