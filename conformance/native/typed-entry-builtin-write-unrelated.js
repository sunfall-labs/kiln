// kiln-native: profile=native-release
//
// Realms and concurrency issue 20. A store onto a built-in the typed bodies below do not rely on
// -- another member of Math, another built-in, an array prototype member -- must neither stop the
// program (a typed entry's failed built-in guard used to be a fatal refusal: "CallMethod
// invocation refused") nor change an answer. The first line is the issue's repro verbatim.
Math.foo = 1; // any store onto any built-in's own-property store
class Point { constructor(x, y) { this.x = x; this.y = y; }
               dist() { return Math.sqrt(this.x * this.x + this.y * this.y); } }
const pts = []; for (let i = 0; i < 1000; i++) pts.push(new Point(i, i + 1));
let s = 0; for (const p of pts) s += p.dist();
console.log(s);

JSON.extra = "x";
Number.helper = (n) => n + 1;
Array.prototype.last = function () { return this[this.length - 1]; };

// A direct call entered typed, a typed callback in `map` and `reduce`, and a typed method.
function hyp(a, b) { return Math.sqrt(a * a + b * b) + Math.floor(a / 3) + Math.abs(b - a); }
let h = 0;
for (let i = 0; i < 500; i++) h += hyp(i, i * 0.5);
const roots = [1, 4, 9, 16, 25, 36].map((x) => Math.sqrt(x) * 2);
const total = roots.reduce((acc, x) => acc + Math.max(x, 3), 0);
class Shape { constructor(r) { this.r = r; } area() { return Math.PI * Math.pow(this.r, 2); } }
let a = 0;
for (let i = 0; i < 100; i++) a += new Shape(i % 7).area();
console.log(h, roots.join(","), total, a.toFixed(6), roots.last(), Number.helper(1), JSON.extra, Math.foo);
