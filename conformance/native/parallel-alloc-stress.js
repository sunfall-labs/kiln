// kiln-native: profile=native-release
//
// Tier 3 (docs/issues/parallel-builtins issue 08) for `mise run native-stress`: the corpus's
// `parallelism=` probes are skipped under collector stress (too large to collect at every
// allocation), so this one is small, has no thread-count directive, and checks its answers
// without allocating. What stress proves here: nothing collects while a call's worker arenas are
// open (the collector aborts if it would), and every value the workers built is reachable through
// the result from the first allocation after the join -- a row whose arena was freed or never
// adopted reads back as garbage, and the comparisons below see it.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 2500 : 20000;
const rows = [];
for (let i = 0; i < n; i++) rows.push({ id: i, qty: i % 7, price: (i % 97) + 0.25 });

let ok = true;
const enriched = rows.parallelMap(r => ({ ...r, total: r.qty * r.price, tag: "r" + r.id }));
const again = rows.map(r => ({ ...r, total: r.qty * r.price, tag: "r" + r.id }));
for (let i = 0; i < n; i++) {
  const a = enriched[i], b = again[i];
  ok = ok && a.id === b.id && a.total === b.total && a.tag === b.tag && a.price === b.price;
}
console.log("enriched rows", ok);

const pairs = rows.parallelFlatMap(r => [r.id, "q" + r.qty]);
ok = pairs.length === 2 * n;
for (let i = 0; i < n; i++) ok = ok && pairs[2 * i] === i && pairs[2 * i + 1] === "q" + (i % 7);
console.log("flat pairs", ok);

// Flattening removes only the callback's outer array. Retain its inner arrays while a
// sequential replay allocates new arrays and collects, then read every retained child.
const nestedOnce = r => [[r.id], r.id];
const nested = rows.parallelFlatMap(nestedOnce);
const nestedReplay = rows.flatMap(nestedOnce);
ok = nested.length === 2 * n && nestedReplay.length === nested.length;
for (let i = 0; i < n; i++) {
  ok = ok && nested[2 * i][0] === i && nested[2 * i + 1] === i;
  ok = ok && nestedReplay[2 * i][0] === i && nestedReplay[2 * i + 1] === i;
}
console.log("retained flat children", ok);

const kept = rows.parallelFilter(r => ("k" + r.qty).length > 1 && r.qty > 3);
ok = true;
for (let i = 0, k = 0; i < n; i++) if (rows[i].qty > 3) ok = ok && kept[k++] === rows[i];
console.log("kept rows", ok);

// Built values survive the collections that follow the join, over and over.
ok = true;
for (let round = 0; round < 3; round++) {
  const built = rows.parallelMap(r => ({ at: r.id, pair: [r.qty, r.price] }));
  for (let i = 0; i < n; i++) ok = ok && built[i].at === i && built[i].pair[0] === rows[i].qty && built[i].pair[1] === rows[i].price;
}
console.log("rounds", ok);
