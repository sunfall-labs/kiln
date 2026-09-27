// native-env: KILN_PARALLEL_ARENA_KIB=256
// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 3 (.scratch/parallel-builtins issue 08): a worker arena that runs out of its budget
// declines like any other worker, and the call runs the sequential method from its first element.
// Each arena here may take one chunk (256 KiB): the small call fits, the large one does not.
// Both answers must be `map`'s -- memory pressure is never a failure -- and repeated calls leave
// nothing behind.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 3000 : 60001;
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = JSON.stringify(p[i]) === JSON.stringify(m[i]);
  console.log(label, JSON.stringify(p[300]), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}
const rows = [];
for (let i = 0; i < n; i++) rows.push({ id: i, qty: i % 7, price: (i % 97) + 0.25 });
const enrich = r => ({ ...r, total: r.qty * r.price, label: "row " + r.id });
check("past the budget", rows.parallelMap(enrich), rows.map(enrich));
const small = rows.slice(0, 600);
check("within it", small.parallelMap(enrich), small.map(enrich));
let last;
for (let k = 0; k < 200; k++) last = small.parallelMap(enrich);
check("repeated", last, small.map(enrich));
