// FOUND BY `soak --node-screen --cases 4000 --seed 101`. Reduced by hand from a 25-line generated
// program.
//
// **The interpreter does not consult `Symbol.toPrimitive`.** The native backend does. So this is
// also an interpreter/native divergence, and the two-engine comparison could in principle have
// found it -- but it never got the chance, because the native surface refuses most programs and
// nobody had written a corpus entry with a `Symbol.toPrimitive` in it. node runs everything.
//
// ToPrimitive's first step is to look up `@@toPrimitive` on the value and, if it is callable, call
// it with the hint and use whatever it returns. The interpreter skips that step entirely and goes
// straight to OrdinaryToPrimitive, which tries `valueOf` and then `toString` -- neither of which
// this object has, so it falls all the way through to `Object.prototype.toString`.
//
//                              node   interpreter        native
//   Number(o)                  5      NaN                5
//   String(o)                  S      [object Object]    S
//   o + 1                      6      [object Object]1   6
//   `t${o}`                    tS     t[object Object]   tS
//   o < 1                      false  false              false
//
// The last row is the one that makes this dangerous rather than merely wrong: relational
// comparison happens to agree, so a corpus that only tested `<` would report a pass.
//
// Verify with:
//   cd fuzz && cargo run --release --bin differ -- findings/symbol-to-primitive-is-ignored.js
const o = { [Symbol.toPrimitive]: (hint) => (hint === "string" ? "S" : 5) };
console.log(Number(o));
console.log(String(o));
console.log(o + 1);
console.log(`t${o}`);
console.log(o < 1);
