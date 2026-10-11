// A method-call site remembers the receiver's shape and the prototype path to the callee.
// Every way that memory can go stale must miss: each section below mutates the world under a
// hot site and prints what the call answered. node is the oracle.

class Point {
  x: number;
  y: number;
  constructor(x: number, y: number) { this.x = x; this.y = y; }
  dist(): number { return Math.sqrt(this.x * this.x + this.y * this.y); }
  tag(): string { return "P"; }
}
class Point3D extends Point {
  z: number;
  constructor(x: number, y: number, z: number) { super(x, y); this.z = z; }
  dist(): number { return Math.sqrt(this.x * this.x + this.y * this.y + this.z * this.z); }
}
class Point4D extends Point3D {
  w = 1;
  tag(): string { return "P4"; }
}

function total(items: Point[]): number {
  let sum = 0;
  for (let i = 0; i < items.length; i++) sum += items[i].dist();
  return sum;
}
function tags(items: Point[]): string {
  let out = "";
  for (let i = 0; i < items.length; i++) out += items[i].tag();
  return out;
}

// 1. polymorphic receivers (two, then three, then more than a site keeps)
const pts: Point[] = [];
for (let i = 0; i < 8; i++) pts.push(i % 2 === 0 ? new Point(i, i + 1) : new Point3D(i, i + 1, i + 2));
console.log("poly2", total(pts), total(pts), tags(pts));
pts.push(new Point4D(1, 2, 3));
console.log("poly3", total(pts), tags(pts));
for (let i = 0; i < 12; i++) {
  const o: any = new Point(i, i);
  o["extra" + i] = i; // a distinct shape each
  pts.push(o);
}
console.log("megamorphic", total(pts), tags(pts));

// 2. prototype method replaced mid-loop
const a = new Point(3, 4);
const seen: number[] = [];
for (let i = 0; i < 6; i++) {
  if (i === 3) (Point.prototype as any).dist = function (this: Point) { return -1; };
  seen.push(a.dist());
}
console.log("replaced", seen.join(","));
Object.defineProperty(Point.prototype, "dist", {
  value: function (this: Point) { return Math.sqrt(this.x * this.x + this.y * this.y); },
  writable: true, configurable: true,
});
console.log("restored", a.dist());

// 3. an own property shadows the prototype method; deleting it unshadows
const b = new Point(6, 8);
const shadow: string[] = [];
for (let i = 0; i < 6; i++) {
  if (i === 2) (b as any).dist = () => 99;
  if (i === 4) delete (b as any).dist;
  shadow.push(String(b.dist()));
}
console.log("shadow", shadow.join(","));

// 4. two objects of one shape: an own method is read from this object, not remembered by value
function make(n: number): any { return { n, f() { return n; } }; }
const objs = [make(1), make(2), make(3)];
console.log("own-per-object", objs.map((o) => o.f()).join(","));

// 5. Object.setPrototypeOf mid-loop, to a sibling and to null
const c: any = new Point3D(1, 2, 2);
const protos: string[] = [];
for (let i = 0; i < 6; i++) {
  if (i === 2) Object.setPrototypeOf(c, Point.prototype);
  if (i === 4) c.__proto__ = { dist() { return "other"; } };
  protos.push(String(c.dist()));
}
console.log("setPrototypeOf", protos.join(","));
const d: any = new Point(3, 4);
let threw = "none";
for (let i = 0; i < 4; i++) {
  if (i === 2) Object.setPrototypeOf(d, null);
  try { d.dist(); } catch (e: any) { threw = e instanceof TypeError ? "TypeError" : "other"; }
}
console.log("proto-null", threw);

// 6. a prototype further up gets the method after the site learned "absent"/"not found here"
class Base {}
class Mid extends Base {}
const m: any = new Mid();
const late: string[] = [];
for (let i = 0; i < 6; i++) {
  if (i === 3) (Base.prototype as any).hello = () => "hi";
  try { late.push(m.hello()); } catch { late.push("TypeError"); }
}
console.log("late-method", late.join(","));
class Mid2 extends Base { hello() { return "mid"; } }
const m2: any = new Mid2();
const shadowUp: string[] = [];
for (let i = 0; i < 4; i++) {
  if (i === 2) delete (Mid2.prototype as any).hello;
  shadowUp.push(m2.hello());
}
console.log("delete-up", shadowUp.join(","));

// 7. getter-defined methods
class G {
  n = 0;
  get run() { this.n++; return () => "got" + this.n; }
}
const g = new G();
const gs: string[] = [];
for (let i = 0; i < 4; i++) gs.push((g as any).run());
console.log("getter", gs.join(","));
const hg: any = { get m() { return function () { return "g1"; }; } };
const hs: string[] = [];
for (let i = 0; i < 4; i++) {
  if (i === 2) Object.defineProperty(hg, "m", { get() { return function () { return "g2"; }; } });
  hs.push(hg.m());
}
console.log("getter-redefined", hs.join(","));

// 8. Proxy receivers at a site that learned plain objects
const target = new Point(3, 4);
const prox: any = new Proxy(target, {
  get(t: any, k: any, r: any) { return k === "dist" ? () => "trapped" : Reflect.get(t, k, r); },
});
const mix: any[] = [new Point(3, 4), prox, new Point3D(0, 3, 4), prox];
const ps: string[] = [];
for (let r = 0; r < 3; r++) for (const o of mix) ps.push(String(o.dist()));
console.log("proxy", ps.join(","));

// 9. TypeScript types lie
const liars: any[] = [new Point(3, 4), { dist: () => "liar" }, new Point3D(1, 2, 2), Object.create(null)];
liars[3].dist = () => "nullproto";
const ls: string[] = [];
for (let r = 0; r < 2; r++) for (const o of liars as Point[]) ls.push(String(o.dist()));
console.log("as-any", ls.join(","));
const notThere: any = { };
let missing = "none";
try { (notThere as Point).dist(); } catch (e: any) { missing = e instanceof TypeError ? "TypeError" : "other"; }
console.log("not-a-function", missing);

// 10. the receiver changes between argument evaluation and the call
const swap = new Point(3, 4);
function swapAndGet(): number {
  (Point.prototype as any).dist = function () { return 7; };
  return 1;
}
const orig = Object.getOwnPropertyDescriptor(Point.prototype, "dist")!;
(swap as any).scale = function (k: number) { return "orig" + k; };
const before = (swap as any).scale(swapAndGet());
console.log("arg-eval", before, swap.dist());
Object.defineProperty(Point.prototype, "dist", orig);
console.log("done", swap.dist());
