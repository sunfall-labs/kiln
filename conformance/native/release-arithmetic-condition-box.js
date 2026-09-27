// kiln-native: profile=native-release parallelism=1,2,max
//
// A branch on a *comparison* of two proven numbers tests their twins, but a branch on an
// *arithmetic* value -- `i % 2 ? a : b`, `n - 1 ? .. : ..`, `while (n % 7)` -- tests the value's
// truthiness through its box. The box analysis treated every twin-only operator as a condition
// that reads no box, so the fast clone and the typed entry dropped the box and branched on a
// variable nothing wrote: every such condition took one arm (issue 27). `+x` of a proven number
// copies `x`'s box, and was treated the same way.

const shared = [1, , 3];
const xs = [10, 20, 30, 40];
// The issue's example: a two-parameter callback branching on the index's remainder.
console.log(JSON.stringify(xs.map((x, i) => i % 2 ? shared : [x])));
console.log(JSON.stringify(xs.map((x, i) => i % 2 ? "a" : "b")), JSON.stringify(xs.map((x) => x % 20 ? "a" : "b")));

// A named function, and every twin-only arithmetic operator as a condition.
function parity(i) { return i % 2 ? "odd" : "even"; }
console.log(parity(0), parity(1), parity(-3), parity(-4), parity(2.5), parity(NaN));
function minusOne(n) { return n - 1 ? "t" : "f"; }
function plusOne(n) { return n + 1 ? "t" : "f"; }
function timesZero(n) { return n * 0 ? "t" : "f"; }
function quotient(a, b) { if (a / b) return "t"; return "f"; }
function square(n) { return n ** 2 ? "t" : "f"; }
console.log(minusOne(1), minusOne(2), plusOne(-1), plusOne(0), timesZero(3), quotient(1, 2), quotient(0, 2), quotient(0, 0), square(0), square(2));
function nested(a) { return (a * 2) ? (a % 2 ? 1 : 2) : 3; }
console.log(nested(0), nested(1), nested(2), nested(-0));
function negativeZero(a) { const t = a - 2; return [t ? 1 : 0, +t]; }
console.log(JSON.stringify([negativeZero(2), negativeZero(NaN), negativeZero(-0 + 2)]));

// Loop conditions: a wrong answer here never terminates.
function untilMultiple(n) { let c = 0; while (n % 7) { n++; c++; } return c; }
function countDown(n) { let c = 0; do { n--; c++; } while (n - 2); return c; }
console.log(untilMultiple(1), untilMultiple(7), untilMultiple(-3), countDown(5));

// Callbacks with an index parameter, in every method that tests the answer's truthiness.
const ys = [1, 2, 3, 4, 5];
console.log(JSON.stringify([
  ys.filter((x, i) => i % 2), ys.filter((x) => x - 3), ys.findIndex((x, i) => i % 3 ? false : i > 0),
  ys.some((x, i) => i - 4), ys.every((x, i) => i + 1), ys.find((x) => x * 0), ys.findLast((x, i) => i % 2),
  ys.findLastIndex((x) => x % 2),
]));
console.log(JSON.stringify([
  ys.map((x, i) => i % 2 ? x : -x), ys.reduce((acc, x, i) => i % 2 ? acc + x : acc, 0),
  ys.flatMap((x, i) => i % 2 ? [] : [x, x]), ys.map((x) => +(x * 2)),
]));
let sum = 0;
ys.forEach((x, i) => { if (i % 2) sum += x; });
console.log(sum);

// `+x` of a product: the result's box is a copy of the product's.
function unaryPlus(a) { const t = a * 2; const u = +t; return [u]; }
function unaryPlusInt(a) { const t = (a >>> 0) % 5; const u = +t; return [u]; }
console.log(JSON.stringify([unaryPlus(1), unaryPlus(2.5), unaryPlusInt(7), unaryPlusInt(9)]));

// The parallel family runs the same callbacks through its typed clones.
const zs = [];
for (let i = 0; i < 20001; i++) zs.push(i * 1.5);
function same(label, p, m) { console.log(label, JSON.stringify(p) === JSON.stringify(m) ? "same" : "DIFFERENT", JSON.stringify(p.slice(0, 4))); }
const pick = (x, i) => i % 2 ? [7, 8] : [x];
same("parallelFlatMap i % 2", zs.parallelFlatMap(pick), zs.flatMap(pick));
const flip = (x, i) => i % 3 ? x : -x;
same("parallelMap i % 3", zs.parallelMap(flip), zs.map(flip));
const odd = (x) => x % 2 ? 1 : 0;
same("parallelMap x % 2", zs.parallelMap(odd), zs.map(odd));
const keep = (x, i) => i % 4;
same("parallelFilter i % 4", zs.parallelFilter(keep), zs.filter(keep));
