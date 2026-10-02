// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 4 (docs/issues/parallel-builtins issue 09), numbers: `Number(x)`, `parseInt`, `parseFloat`,
// `Boolean`, `String`, `Number.isInteger` and friends, `toFixed` and `toString` -- the number
// parsing and formatting a callback calls -- run on the pool and answer exactly what `map`
// answers, `-0` and `NaN` included. `toFixed` and `parseInt`/`parseFloat` share their digits
// with the sequential definitions (`kiln_c99_fixed_spelling`, `kiln_c99_parse_*_bytes`).
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1501 : 30001;
function same(a, b) {
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!same(a[i], b[i])) return false;
    return true;
  }
  return Object.is(a, b);
}
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = same(p[i], m[i]);
  console.log(label, p.length, JSON.stringify(p[3]), JSON.stringify(p[p.length - 1]), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}

const texts = ["12", "  -7.5 ", "\u00a0 42\u00a0", "é5", "5\u00a0\u00a01", "0x1f", "1e3", "", "12px", "abc", "Infinity", "-0", ".5", "5.", "1_000", "0b11", "  +3e-2x"];
const strs = [];
const nums = [];
for (let i = 0; i < n; i++) {
  strs.push(texts[i % texts.length]);
  nums.push(i % 11 === 0 ? -0 : (i % 17 === 0 ? NaN : (i - n / 2) * 1.0371));
}
const parse = s => [Number(s), parseInt(s), parseInt(s, 16), parseInt(s, 36), parseInt(s, 1), parseFloat(s), Number.parseFloat(s), Number.parseInt(s, 8), Boolean(s), String(s)];
check("parse strings", strs.parallelMap(parse), strs.map(parse));
check("map(parseInt) passes the index as a radix", strs.parallelMap(s => [s].map(parseInt)[0] + ["10", "11"].map(parseInt).join()), strs.map(s => [s].map(parseInt)[0] + ["10", "11"].map(parseInt).join()));
// `Number` of UTF-16 text: white space trimmed, anything else non-ASCII NaN. (`parseInt` and
// `parseFloat` of UTF-16 text fail natively, sequentially too -- issue 30 -- so they are not here.)
const wide = ["\u2003 7 \u3000", "7日", "日", "\ufeff-1e2\u2028"];
const wides = strs.map((s, i) => i % 3 === 0 ? wide[i % wide.length] : s);
const toNumber = s => Number(s);
check("Number of UTF-16 text", wides.parallelMap(toNumber), wides.map(toNumber));
const format = (x, i) => [x.toFixed(2), x.toFixed(0), x.toFixed(i % 21), x.toString(), x.toString(10), String(x), (x * 1e20).toFixed(1), (1.005).toFixed(2), (-0.0004).toFixed(2)];
check("format numbers", nums.parallelMap(format), nums.map(format));
const tests = x => [Number.isInteger(x), Number.isSafeInteger(x * 1e14), Number.isNaN(x), Number.isFinite(x / 0), Number(x), Number(), Boolean(x), Math.round(x) + Math.max(x, 1)];
check("number statics", nums.parallelMap(tests), nums.map(tests));
const total = s => s.split("").map(Number).filter(d => !Number.isNaN(d)).length;
check("digits", strs.parallelMap(total), strs.map(total));
// A key comparator that parses its key: the pool computes every key once and sorts by it.
const ids = [];
for (let i = 0; i < n; i++) ids.push(" " + ((i * 7919) % n) + "px");
const byId = (a, b) => parseInt(a) - parseInt(b);
check("sort by a parsed key", ids.slice().parallelSort(byId), ids.slice().sort(byId));
