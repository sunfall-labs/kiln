// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 4 (.scratch/parallel-builtins issue 09), string methods: a callback may call the string
// methods marked pure in the member description -- trim and its halves, split (a string
// separator, a limit, the empty separator), slice, substring, charAt, at, codePointAt, indexOf,
// lastIndexOf, includes, startsWith, endsWith, the ASCII case maps, padStart/padEnd, repeat,
// toString -- and run on the pool, answering exactly what `map` answers. Inputs mix Latin1,
// UTF-16 and astral text, so a worker path that declines (a non-ASCII case map) and one that
// answers (a UTF-16 split) are both exercised; either way the answer is `map`'s.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1501 : 30001;
function same(a, b) {
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (i in a !== i in b || !same(a[i], b[i])) return false;
    return true;
  }
  return Object.is(a, b);
}
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = same(p[i], m[i]);
  console.log(label, p.length, JSON.stringify(p[1]), JSON.stringify(p[p.length - 1]), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}

const words = ["  apple, 1 ,2", "\tBanana,3,,4\n", "cherry", "", " ", "date,5.5,-6", "éclair,7", "日本,語", "𝒽ello,8", "a,b,c,d,e"];
const lines = [];
for (let i = 0; i < n; i++) lines.push(words[i % words.length] + (i % 3 === 0 ? "," + i : ""));

// The acceptance shape: parse a CSV line into numbers.
const csv = l => l.trim().split(",").map(Number);
check("csv", lines.parallelMap(csv), lines.map(csv));
const trims = l => [l.trim(), l.trimStart(), l.trimEnd()];
check("trim", lines.parallelMap(trims), lines.map(trims));
const splits = l => [l.split(","), l.split(", "), l.split(""), l.split(), l.split("語"), l.split(",", -1)];
check("split", lines.parallelMap(splits), lines.map(splits));
// A limit, over Latin1 lines only: the native `split` of a UTF-16 string ignores its limit
// (issue 30), sequentially as well, so there the two would differ from the interpreter together.
const latin = lines.filter(l => !l.includes("日") && !l.includes("𝒽"));
const limits = l => [l.split(",", 2), l.split(",", 0), l.split(",", 1), l.split("", 3), l.split(undefined, 0)];
check("split with a limit", latin.parallelMap(limits), latin.map(limits));
const slices = (l, i) => [l.slice(1), l.slice(-3), l.slice(2, -1), l.slice(5, 2), l.substring(3, 1), l.substring(-2), l.substring(1, i % 7)];
check("slice and substring", lines.parallelMap(slices), lines.map(slices));
const units = l => [l.charAt(0), l.charAt(99), l.at(-1), l.at(0), l.at(50), l.codePointAt(0), l.codePointAt(1), l.codePointAt(80)];
check("charAt, at, codePointAt", lines.parallelMap(units), lines.map(units));
const search = (l, i) => [l.indexOf(","), l.indexOf(",", 3), l.indexOf(""), l.lastIndexOf(","), l.lastIndexOf(",", 2), l.lastIndexOf("", NaN), l.includes("a"), l.includes(i % 10), l.startsWith(" "), l.startsWith("an", 2), l.endsWith("2"), l.endsWith("a", 4)];
check("search", lines.parallelMap(search), lines.map(search));
const cases = l => [l.toUpperCase(), l.toLowerCase(), l.toString()];
check("case maps", lines.parallelMap(cases), lines.map(cases));
const pads = (l, i) => [l.padStart(12, "*-"), l.padEnd(9), l.padStart(i % 5, "x"), l.padEnd(20, ""), l.trim().repeat(i % 3)];
check("pad and repeat", lines.parallelMap(pads), lines.map(pads));
const fields = l => l.split(",").map(f => f.trim()).filter(f => f.length > 0).join("|");
check("fields", lines.parallelMap(fields), lines.map(fields));
const keep = l => l.trim().startsWith("a") || l.includes("5");
check("filter", lines.parallelFilter(keep), lines.filter(keep));
