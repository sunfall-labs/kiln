// `**`, `**=` and Math.pow are ECMA-262 Number::exponentiate, which differs from C's pow (and
// Rust's powf) at a few edges: a NaN exponent, and an infinite exponent with a base of magnitude
// one, are NaN where C answers 1. Each path that computes exponentiation -- constant folding,
// the generic operator, the typed loop, integer operands, compound assignment, Math.pow -- must
// agree with node.
const show = (v) => (Object.is(v, -0) ? "-0" : String(v));
console.log("folded", show(1 ** NaN), show((-1) ** Infinity), show(1 ** -Infinity), show(2 ** 10), show((-8) ** (1 / 3)));
const bases = [1, -1, 2, 0, -0, NaN, Infinity, -Infinity, 0.5];
const exponents = [NaN, Infinity, -Infinity, 0, -0, 0.5, -1, 3];
for (const e of exponents) {
  const generic = bases.map((b) => show(b ** e)).join(",");
  const math = bases.map((b) => show(Math.pow(b, e))).join(",");
  console.log(show(e), generic, generic === math);
}
let sum = 0;
for (let i = 0; i < 1000; i++) sum += (i % 3 - 1) ** ((i % 2) ? NaN : Infinity);
console.log("typed loop", show(sum));
let ints = 0;
for (let i = 0; i < 10; i++) ints += (i | 0) ** (2 | 0);
console.log("ints", ints);
let x = 1;
x **= NaN;
let y = -1;
y **= -Infinity;
console.log("compound", show(x), show(y));
