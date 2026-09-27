// kiln-native: profile=native-release
//
// Realms and concurrency issue 20. A typed body relies on a built-in member (an inlined
// `Math.sqrt`); when the program writes that member, every call must see the override -- through
// the boxed body when the write came first, and through the general call from inside the typed
// entry when the call itself made it -- and the original again once it is put back.
const original = Math.sqrt;
Math.sqrt = (x) => -1;
class Point { constructor(x, y) { this.x = x; this.y = y; }
               dist() { return Math.sqrt(this.x * this.x + this.y * this.y); } }
const pts = []; for (let i = 0; i < 1000; i++) pts.push(new Point(i, i + 1));
let s = 0; for (const p of pts) s += p.dist();
console.log("patched first", s);

function hyp(a, b) { return Math.sqrt(a * a + b * b); }
const phases = [];
function run(label) {
  let h = 0;
  for (let i = 0; i < 200; i++) h += hyp(i, i + 2);
  let d = 0;
  for (const p of pts) d += p.dist();
  const mapped = [1, 4, 9].map((x) => Math.sqrt(x));
  const folded = [1, 4, 9, 16].reduce((acc, x) => acc + Math.sqrt(x), 0);
  const view = new Float64Array([4, 9, 16]).map((x) => Math.sqrt(x));
  phases.push(`${label}: ${h} ${d} ${mapped.join(",")} ${folded} ${Array.from(view).join(",")}`);
}
run("still patched");
Math.sqrt = original;
run("restored");
Math.sqrt = (x) => x * 10;
run("patched again");
Math.sqrt = original;
run("restored again");
console.log(phases.join("\n"));

// A patch answering a string: the typed entry was compiled for a number, and the boxed call
// the site makes instead hands the string on.
Math.sqrt = (x) => "r" + x;
const strings = pts.slice(0, 3).map((p) => p.dist());
console.log(strings.join(" "), "" + hyp(3, 4));
Math.sqrt = original;

// The call itself modifies the member it relies on: the typed entry is past its callers' check,
// so it takes the general call from where it is, after the effect, without repeating it.
let patches = 0;
function flip(x) { if (x === 5) { patches++; Math.sqrt = (v) => v * 1000; } return 0; }
function rooted(x) { const k = flip(x); return Math.sqrt(x) + k; }
let r = 0;
for (let i = 0; i < 10; i++) r += rooted(i);
Math.sqrt = original;
class Rooted { constructor(x) { this.x = x; } m() { flip(this.x); return Math.sqrt(this.x); } }
let q = 0;
for (let i = 0; i < 10; i++) q += new Rooted(i).m();
Math.sqrt = original;
console.log(r, q, patches);
