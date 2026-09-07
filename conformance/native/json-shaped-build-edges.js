// The corners of the JSON fast paths that stopped building an intermediate boundary value for
// every parsed string and every written document: `kiln_c99_json_build` handing a `kiln_val`
// straight to `kiln_c99_json_define` / `kiln_c99_array_push_val`, `kiln_c99_text_val` answering
// the stringify builder in one allocation, the fused quote-and-copy in `kiln_c99_json_quote`,
// and the scalar pre-dispatch in `kiln_c99_json_scalar`.
//
// Every line below was checked against node as well as against the interpreter. The one
// deliberate exception is named where it appears.

// --- keys: escaped and not, duplicated, `__proto__`, and the ones a shape stores ---
console.log(JSON.stringify({ a: 1, "b c": 2, 'q"q': 3, "back\\slash": 4, "nl\nnl": 5 }));
const dup = JSON.parse('{"k":1,"k":2,"k":3}');
console.log(JSON.stringify(dup), Object.keys(dup).length);
const proto = JSON.parse('{"__proto__":{"polluted":1},"ok":2}');
console.log(
  Object.keys(proto).join(","),
  Object.getPrototypeOf(proto) === Object.prototype,
  {}.polluted,
  JSON.stringify(proto),
);
// An integer-like key and a string key that only differ by a leading zero, both kept, both
// spelled back. `{"":1,"0":2}` -- a *string* key before an integer-like one -- is left out
// deliberately: this backend's `JSON.parse` leaves an integer-like key in the shape rather than
// in the element store, so it spells such an object in source order where node and the
// interpreter put the index first. That is a pre-existing divergence of its own, unrelated to
// anything here, and this corpus is not the place to discover it.
console.log(JSON.stringify(JSON.parse('{"0":2,"01":3,"a":4}')));

// --- every escape a JSON string can spell, in both directions ---
const escapes = JSON.parse('"\\"\\\\\\/\\b\\f\\n\\r\\t\\u0041\\u00e9\\u20ac\\ud83d\\ude00"');
console.log(escapes.length, JSON.stringify(escapes));
console.log(JSON.parse('"\\u0000\\u001f"').length, JSON.stringify(JSON.parse('"\\u0000\\u001f"')));

// A lone surrogate escape is the ONE line here that node and kiln answer differently, on purpose:
// a kiln string cannot hold an unpaired surrogate, so ADR 0022 decodes one to U+FFFD where node
// keeps the surrogate. The two kiln engines must still agree with each other, which is what this
// corpus asks; the label keeps the disagreement with node to this line.
console.log(JSON.parse('"\\ud800"').codePointAt(0) === 0xfffd ? "replacement" : "surrogate");

// --- numbers at the edges ---
console.log(JSON.stringify([0, -0, 1e21, 1e-7, 5e-324, 1.7976931348623157e308, 1e300 * 1e300]));
console.log(JSON.stringify([9007199254740991, 9007199254740993, 18446744073709551616, -0.5]));
console.log(Object.is(JSON.parse("-0"), -0), Object.is(JSON.parse("0"), -0));
console.log(JSON.parse("[1e400,-1e400,1e-400,123456789012345678901234567890]").join("|"));
console.log(JSON.stringify({ n: NaN, i: Infinity, m: -Infinity }));
console.log(JSON.stringify([98.5, 87.2, 94.0, 100.0, 76.8, 1042, 4]));

// --- what has no JSON spelling, in an object versus in an array ---
const missing = { a: undefined, b: function () {}, c: Symbol("s"), d: 1 };
console.log(JSON.stringify(missing), JSON.stringify([undefined, function () {}, Symbol("s"), 1]));
console.log(String(JSON.stringify(undefined)), String(JSON.stringify(function () {})));
const sparse = [1, , 3];
sparse[6] = 7;
console.log(JSON.stringify(sparse), sparse.length);

// --- toJSON, on the value and on a member ---
const withToJSON = {
  toJSON() {
    return { replaced: true };
  },
};
console.log(
  JSON.stringify(withToJSON),
  JSON.stringify({ m: withToJSON }),
  JSON.stringify([withToJSON]),
);
console.log(JSON.stringify({ d: new Date(0) }));
console.log(
  JSON.stringify({
    toJSON() {
      return undefined;
    },
  }) === undefined,
);
console.log(
  JSON.stringify({
    a: {
      toJSON() {
        return undefined;
      },
    },
    b: 1,
  }),
);

// --- the space argument, as a number and as a string ---
const shaped = { id: 1042, tags: ["prod", "edge"], meta: { r: 4 } };
console.log(JSON.stringify(shaped, null, 2));
console.log(JSON.stringify(shaped, null, "\t"));
console.log(
  JSON.stringify(shaped, null, 20).length,
  JSON.stringify(shaped, null, "abcdefghijklmnop").length,
);
console.log(JSON.stringify([], null, 2), JSON.stringify({}, null, 2), JSON.stringify([[]], null, 2));

// --- deep nesting, past the walk's local frame array ---
let deep = 0;
for (let i = 0; i < 200; i++) deep = { d: deep };
const deepText = JSON.stringify(deep);
console.log(deepText.length, JSON.stringify(JSON.parse(deepText)) === deepText);
let deepArr = [];
for (let i = 0; i < 200; i++) deepArr = [deepArr];
const deepArrText = JSON.stringify(deepArr);
console.log(deepArrText.length, JSON.stringify(JSON.parse(deepArrText)) === deepArrText);

// --- very long strings and very long arrays ---
const long = "x".repeat(5000) + '"' + "y".repeat(5000) + "\\" + "z".repeat(5000);
const longText = JSON.stringify(long);
console.log(longText.length, JSON.parse(longText) === long);
const clean = "abcdefgh".repeat(2000);
console.log(JSON.parse(JSON.stringify(clean)) === clean, JSON.stringify(clean).length);
const big = [];
for (let i = 0; i < 5000; i++) big.push(i % 7 === 0 ? String(i) : i / 4);
const bigText = JSON.stringify(big);
const bigBack = JSON.parse(bigText);
console.log(bigText.length, bigBack.length, JSON.stringify(bigBack) === bigText);
const bigObj = {};
for (let i = 0; i < 500; i++) bigObj["k" + i] = i % 3 === 0 ? { i } : "v" + i;
const bigObjText = JSON.stringify(bigObj);
console.log(bigObjText.length, JSON.stringify(JSON.parse(bigObjText)) === bigObjText);

// --- a round trip of everything at once ---
const everything = {
  s: "plain",
  esc: 'a"b\\c\nd\te',
  n: [0, -0, 1, -1.5, 1e21, 1e-7, 5e-324],
  b: [true, false],
  z: null,
  empty: { e: {}, a: [] },
  nested: { deep: { deeper: [{ k: "v" }, [1, [2, [3]]]] } },
  uni: "héllo — 😀 ✓",
};
const once = JSON.stringify(everything);
const twice = JSON.stringify(JSON.parse(once));
console.log(once === twice, once.length);
console.log(twice);
