// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 3 (.scratch/parallel-builtins issue 08): callbacks that build strings -- `+` with a string
// on either side, template literals, `String(x)` -- over numbers of every spelling (integers,
// fractions, -0, NaN, Infinity, exponents), booleans, null, undefined and strings of both widths.
// Each answer must be `map`'s, unit for unit.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1500 : 30001;
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = Object.is(p[i], m[i]);
  console.log(label, JSON.stringify(p[0]), JSON.stringify(p[5]), JSON.stringify(p[11]), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}
const specials = [0, -0, 1.5, -2.25, 1e21, 1.5e-7, 123456789012, NaN, Infinity, -Infinity, 0.1 + 0.2, 2 ** 53];
const words = ["plain", "café", "€uro", "emoji 😀", "", "a\u0000b"];
const values = [];
for (let i = 0; i < n; i++) {
  const k = i % 5;
  values.push(k === 0 ? specials[i % specials.length] : k === 1 ? words[i % words.length] : k === 2 ? i % 2 === 0 : k === 3 ? (i & 1 ? null : undefined) : i * 0.5);
}
const joined = v => "v=" + v;
check("prefix", values.parallelMap(joined), values.map(joined));
const suffix = v => v + "!";
check("suffix", values.parallelMap(suffix), values.map(suffix));
const template = (v, i) => `${i}:${v}|${i & 7}`;
check("template", values.parallelMap(template), values.map(template));
const named = v => String(v);
check("String()", values.parallelMap(named), values.map(named));
const nums = [];
for (let i = 0; i < n; i++) nums.push(specials[i % specials.length] * (i % 3 === 0 ? 1 : i));
const csv = x => x + "," + (x * 2) + "," + (x > 0);
check("csv line", nums.parallelMap(csv), nums.map(csv));
const words2 = [];
for (let i = 0; i < n; i++) words2.push(words[i % words.length] + i);
const wide = w => w + "→" + w.length;
check("wide join", words2.parallelMap(wide), words2.map(wide));
// Numbers alone still add.
const sum = x => x + 1;
check("numbers add", nums.parallelMap(sum), nums.map(sum));
