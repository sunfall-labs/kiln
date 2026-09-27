// kiln-native: profile=native-release
//
// A release build runs Oxc's dead-code pass over every source module, and Oxc's side-effect
// model is a bundler's: it assumes a coercion (`valueOf`, `toString`, `Symbol.toPrimitive`) runs
// no code, that comparisons never throw, and that some global functions are pure whatever their
// arguments. Every statement below has a discarded result and is observable only through the code
// its coercion or its throw runs; each was deleted, inlined out of order, or folded before the
// guard in `kiln-frontend-oxc/src/dce_guard.rs` (issue 19). The guard also covers an array
// literal's element `toString`, computed keys and class heritage; those are left out here because
// the engines disagree on them in every profile (issues filed alongside 19), and the frontend's
// unit tests pin that the DCE keeps them.

const seen = [];
const spy = (name, value) => ({
  valueOf() {
    seen.push(name);
    return value;
  },
  toString() {
    seen.push(name + ":s");
    return String(value);
  },
});

function discarded() {
  const a = spy("lt", 2);
  a < 10;
  a >= 1;
  const b = spy("eq", 3);
  b == 3;
  b != "3";
  b == null; // no coercion: must not log
  +spy("plus", 1);
  -spy("neg", 1);
  ~spy("not", 1);
  spy("add", 1) + 1;
  spy("sub", 1) - 1;
  `${spy("tpl", 1)}`;
  String(spy("String", 1));
  Math.abs(spy("Math.abs", 1));
  isNaN(spy("isNaN", 1));
  parseInt(spy("parseInt", 1));
  new Date(spy("Date", 1));
  const unused = spy("unused-binding", 1) < 1;
}
discarded();
console.log(seen.join(" "));

// A throw is an effect too.
function throws(label, run) {
  try {
    run();
    console.log(label, "no throw");
  } catch (error) {
    console.log(label, error.constructor.name);
  }
}
throws("decodeURIComponent", () => {
  decodeURIComponent("%");
});
throws("decodeURI", () => {
  decodeURI("%zz");
});
throws("optional call", () => {
  decodeURIComponent?.("%");
});
throws("symbol compare", () => {
  Symbol("s") < 1;
});
throws("bigint mix", () => {
  1n + 1;
});
throws("toPrimitive throws", () => {
  ({ valueOf() { throw new RangeError("v"); } }) < 1;
});

// Moving `new Date(spy)` past a read of what its coercion writes is the same mistake.
let hint = "none";
const dated = new Date({
  [Symbol.toPrimitive](h) {
    hint = h;
    return 0;
  },
});
console.log(hint, dated.getTime());

// What the guard must not cost: `x == null`, `===`, and primitive comparisons still fold.
const LIMIT = 3;
let folded = "kept";
if (LIMIT > 5) folded = "dead branch ran";
console.log(folded, typeof discarded === "function");
