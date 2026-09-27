// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 4 (.scratch/parallel-builtins issue 09), arrays: a callback may call `slice`, `join`,
// `indexOf`, `includes`, and `map`, `filter`, `some`, `every` whose own callback the proof covers
// (a closure written in the callback, a function of the program, or a pure global such as
// `Number`) -- over plain arrays, holes included, and answer exactly what `map` answers.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1501 : 20001;
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
  console.log(label, p.length, JSON.stringify(p[2]), JSON.stringify(p[p.length - 1]), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}

const rows = [];
for (let i = 0; i < n; i++) {
  const row = [i % 5, "s" + (i % 3), i % 7 === 0 ? NaN : i / 3, i % 2 === 0, null, undefined];
  if (i % 4 === 0) delete row[1];
  rows.push(row);
}
const pieces = (r, i) => [r.slice(1), r.slice(-2), r.slice(2, 4), r.join(), r.join("-"), r.join(""), r.indexOf(0), r.indexOf(NaN), r.indexOf(null, 2), r.includes(NaN), r.includes(undefined), r.includes(i % 5, -6)];
check("slice, join, indexOf, includes", rows.parallelMap(pieces), rows.map(pieces));
const loops = r => [r.map(x => x === null), r.map(String), r.filter(Boolean), r.some(x => x === "s2"), r.every(x => x !== undefined), r.map(Number)];
check("map, filter, some, every", rows.parallelMap(loops), rows.map(loops));
function scale(x, i) { return x * 10 + i; }
const helper = r => r.slice(0, 3).map(Number).map(scale);
check("a helper as the inner callback", rows.parallelMap(helper), rows.map(helper));
const nested = r => r.slice(0, 2).map(x => [x, [x].map(y => y + 1)]);
check("nested loops", rows.parallelMap(nested), rows.map(nested));
const keepRow = r => r.some(x => x === 3) && r.join().length > 10;
check("filter by some", rows.parallelFilter(keepRow), rows.filter(keepRow));
