// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 3 (docs/issues/parallel-builtins issue 08): `parallelFilter` keeping objects, with
// predicates that build strings and objects along the way (garbage the moment they answer), and a
// `parallelMap` whose answers are the rows themselves. Each answer must be `filter`'s: the same
// objects, in order.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1500 : 30001;
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = p[i] === m[i];
  console.log(label, JSON.stringify(p[0]), JSON.stringify(p[1]), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}
const first = ["ann", "bo", "cyrano", "di"];
const rows = [];
for (let i = 0; i < n; i++) rows.push({ id: i, first: first[i % 4], last: "x" + (i % 11), score: (i * 37) % 101 });
const longName = r => (r.first + " " + r.last).length > 6;
check("built string predicate", rows.parallelFilter(longName), rows.filter(longName));
const tagged = r => `${r.first}-${r.score}` > "c";
check("template compare", rows.parallelFilter(tagged), rows.filter(tagged));
const boxed = r => ({ ok: r.score > 50 }).ok;
check("object predicate", rows.parallelFilter(boxed), rows.filter(boxed));
const keep = r => r;
check("rows themselves", rows.parallelMap(keep), rows.map(keep));
