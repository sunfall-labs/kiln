// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 2 (docs/issues/parallel-builtins issue 07), strings: a callback may read a string's length,
// its code units (`charCodeAt`, and an index below U+0100, which is a static one-unit string),
// compare strings by code units and by value, and use a string as a computed key -- all on the
// pool. An index past U+00FF would allocate a string and runs sequentially; so does a string `+`.
function check(label, p, m) {
  let ok = p.constructor === m.constructor && p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}

const words = ["apple", "Banana", "cherry", "", "date", "éclair", "fig", "grape", "𝒽ello", "kiwi"];
const n = 50003;
const xs = [];
for (let i = 0; i < n; i++) xs.push(words[i % words.length] + (i % 13 === 0 ? "" : ""));
const lengths = w => w.length * 2 + (w.charCodeAt(0) || 0) + (w.charCodeAt(99) || 7) + w.charCodeAt();
check("length and code units", xs.parallelMap(lengths), xs.map(lengths));
const compare = w => (w < "cherry" ? 1 : 0) + (w >= "fig" ? 2 : 0) + (w === "date" ? 4 : 0) + (w != "kiwi" ? 8 : 0);
check("comparisons", xs.parallelMap(compare), xs.map(compare));
const first = w => w[0] === "a" || w[1] === "é";
check("index reads", xs.parallelFilter(first), xs.filter(first));
const wide = w => (w[0] === undefined ? 0 : w[0].length);
check("wide index", xs.parallelMap(wide), xs.map(wide));
const counts = { apple: 1, Banana: 2, cherry: 3, date: 4, fig: 5 };
const lookup = w => counts[w] === undefined ? -1 : counts[w];
check("computed string key", xs.parallelMap(lookup), xs.map(lookup));
const joined = w => (w + "!").length;
check("string + runs sequentially", xs.parallelMap(joined), xs.map(joined));
