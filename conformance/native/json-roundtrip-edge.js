// JSON.stringify/JSON.parse edge cases, exercised against both engines the way the rest of this
// corpus is: the interpreter is the oracle here, and any byte the native backend disagrees on is a
// compiler bug rather than a question about JavaScript.
//
// A replacer and a reviver are both native backend refusals today (unrelated to this change --
// see `KILN_C99_REFUSE("JSON.stringify with a replacer")` and
// `KILN_C99_REFUSE("JSON.parse with a reviver")` in prelude_core.c), so they are covered on their
// own in `json-roundtrip-edge-refuses.js` rather than here: mixing a deliberate refusal into this
// file would make native.sh classify the *whole* file as "refused" the moment it reached that
// line, which would silently stop covering every case below it against a real divergence.

// Every escape JSON defines, plus the C0 controls that have no shorthand, plus a non-BMP
// character as a surrogate pair.
//
// A lone surrogate (one escape of a pair, with no partner) is deliberately not exercised here:
// the interpreter already answers it differently from node for reasons that have nothing to do
// with JSON -- a source literal containing one is mangled at parse time, and `JSON.parse` folds
// an unpaired `\u` escape to U+FFFD rather than keeping the bare surrogate node does -- so a
// probe built on one would fail for a reason unrelated to the two things this file exists to
// check.
console.log(JSON.stringify("quote\" backslash\\ slash/ b\bf\fn\nr\rt\t"));
console.log(JSON.stringify("  control"));
console.log(JSON.stringify("pair 😀 done"));
console.log(JSON.parse('"quote\\" backslash\\\\ slash\\/ b\\bf\\fn\\nr\\rt\\t"'));
console.log(JSON.parse('"\\u0000\\u0001\\u001f control"').length);
console.log(JSON.parse('"pair \\ud83d\\ude00 done"'));

// A long string on both sides -- long enough to force more than one buffer growth, and to make
// the parser's no-escape fast path (a bulk copy rather than one append per byte) and its escaped
// slow path both run on real length rather than a handful of bytes.
const longPlain = "x".repeat(70000);
const longEscaped = "a\\n".repeat(30000);
console.log(JSON.stringify(longPlain).length);
console.log(JSON.parse(JSON.stringify(longPlain)) === longPlain);
console.log(JSON.parse('"' + longEscaped + '"').length);

// Deep nesting on both sides.
let deepArray = 0;
for (let i = 0; i < 100; i++) deepArray = [deepArray];
const deepJson = JSON.stringify(deepArray);
let deepParsed = JSON.parse(deepJson);
let depth = 0;
while (Array.isArray(deepParsed)) { depth++; deepParsed = deepParsed[0]; }
console.log("deep", depth, deepParsed);

let deepObject = { v: 0 };
for (let i = 0; i < 100; i++) deepObject = { n: deepObject };
let deepObjectParsed = JSON.parse(JSON.stringify(deepObject));
let objDepth = 0;
while (deepObjectParsed && typeof deepObjectParsed === "object" && "n" in deepObjectParsed) {
  objDepth++;
  deepObjectParsed = deepObjectParsed.n;
}
console.log("deep-object", objDepth, deepObjectParsed && deepObjectParsed.v);

// Numbers: -0, very large and very small exponents, a value with no exact binary spelling, and
// the three that have no JSON spelling at all.
console.log(JSON.stringify(-0));
console.log(JSON.stringify(1e21));
console.log(JSON.stringify(1e-7));
console.log(JSON.stringify(0.1 + 0.2));
console.log(JSON.stringify(NaN), JSON.stringify(Infinity), JSON.stringify(-Infinity));
console.log(JSON.stringify([NaN, Infinity, -Infinity]));

// `toJSON`, at every level, including inside a nested member -- and a `Date`, which is the
// built-in reason the step exists.
console.log(JSON.stringify({ t: { toJSON() { return "custom"; } } }));
class Money { constructor(c) { this.cents = c; } toJSON() { return "$" + (this.cents / 100); } }
console.log(JSON.stringify({ price: new Money(1250) }));
const fixedDate = new Date(Date.UTC(2020, 0, 2, 3, 4, 5, 6));
console.log(JSON.stringify(fixedDate));
console.log(JSON.stringify({ when: fixedDate }));
console.log(JSON.stringify([fixedDate]));

// `undefined` and a function: dropped from an object, `null` in an array.
console.log(JSON.stringify({ a: undefined, b: function () {}, c: 1 }));
console.log(JSON.stringify([undefined, function () {}, 1]));

// A sparse array: a hole serialises as `null`, exactly like `undefined` at that position.
const sparse = [1, , 3];
console.log(JSON.stringify(sparse));

// The indent argument, by count (capped at 10) and by string (capped at 10 characters).
console.log(JSON.stringify({ a: { b: 1 } }, null, 2));
console.log(JSON.stringify([1, [2]], null, "  "));
console.log(JSON.stringify({ a: 1 }, null, 4));
console.log(JSON.stringify({}, null, 2), JSON.stringify([], null, 2));

// Parse errors: caught, and named by constructor rather than by message text, which is what a
// program actually branches on.
function caught(run) {
  try {
    return "ok:" + JSON.stringify(run());
  } catch (error) {
    return "err:" + error.constructor.name;
  }
}
console.log(caught(() => JSON.parse("{oops")));
console.log(caught(() => JSON.parse("")));
console.log(caught(() => JSON.parse("[1,]")));
console.log(caught(() => JSON.parse("'single'")));
console.log(caught(() => JSON.parse('{"a":1,}')));
console.log(caught(() => JSON.stringify(1n)));

const cyclic = {};
cyclic.self = cyclic;
console.log(caught(() => JSON.stringify(cyclic)));

// The round trip, which is the property every one of the changes above has to preserve.
const original = {
  id: 1042,
  name: "kiln-benchmark",
  active: true,
  scores: [98.5, 87.2, 94, 100, 76.8],
  metadata: { region: "us-east-1", replicas: 4, tags: ["prod", "edge", "wasm"] },
};
console.log(JSON.stringify(JSON.parse(JSON.stringify(original))) === JSON.stringify(original));
console.log(JSON.stringify(JSON.parse(JSON.stringify(original))));
