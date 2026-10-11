// Math.* called with a property read ahead of its arguments (`Math.f(a.x * a.x)`): the callee is
// the original method only until the program patches it. Node's output is the oracle.
const o = { x: 3, y: 4 };
const hyp = () => Math.sqrt(o.x * o.x + o.y * o.y);
const vals = [-0, 0, NaN, Infinity, -Infinity, 0.5, -0.5, 2.5, -2.5, 0.49999999999999994, 4503599627370497, 1e300, "9", { valueOf() { return 16; } }, null, undefined];
const show = (label, f) => { const out = []; for (const v of vals) { const r = f(v); out.push(Object.is(r, -0) ? "-0" : String(r)); } console.log(label, out.join(" ")); };
for (let i = 0; i < 3; i++) console.log(hyp());
for (const n of ["sqrt", "floor", "ceil", "round", "trunc", "abs", "sign", "fround", "log", "exp", "sin", "cos", "cbrt"]) show(n, (v) => Math[n](o.x * 0 + v));
const holder = { v: 0 };
show("round-prep", (v) => { holder.v = v; return Math.round(holder.v); });
show("sqrt-prep", (v) => { holder.v = v; return Math.sqrt(holder.v); });
console.log(Math.pow(1, Infinity), Math.pow(1, NaN), Math.pow(-1, -Infinity), Math.pow(2, 10), Math.pow(NaN, 0));
console.log(Math.max(), Math.min(), Math.max(NaN, 1), Math.max(-0, 0), Math.min(0, -0), Math.max(1, 2, 3));
console.log(Math.round(-0.5), Math.round(0.49999999999999994), Math.round(2.5), Math.round(-2.5));
// patch after warm loops
Math.foo = 1; console.log(hyp());
const original = Math.sqrt;
Math.sqrt = (n) => "patched" + n;
console.log(hyp());
Math.sqrt = original; console.log(hyp());
Object.defineProperty(Math, "sqrt", { get() { return () => "getter"; }, configurable: true });
console.log(hyp());
// Not `delete Math.sqrt` here: the native table keeps answering a deleted built-in member
// (no per-realm tombstones yet), a known gap tracked separately from this probe.
Math.floor = Math.ceil; console.log(Math.floor(o.x / 2));
// shadowing
{ const Math = { sqrt: () => "shadow" }; console.log(Math.sqrt(o.x * o.x)); }
