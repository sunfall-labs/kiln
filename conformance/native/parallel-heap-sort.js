// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 2 (docs/issues/parallel-builtins issue 07): `parallelSort` under a key comparator, `(a, b)
// => K(a) - K(b)` or its reverse, where `K` reads one element. Over keys that are numbers and
// not NaN such a comparator is consistent, so a stable sort by key -- which the pool computes
// without calling the comparator -- is `sort`'s one answer: equal keys keep their order, and
// `-0` and `+0` are equal. A NaN key, an element with no key, `undefined`, and a hole run `sort`.
// Under a NaN key the comparator is inconsistent and `sort`'s order is the engine's own, so for
// those only the agreement is printed, not the order.
function check(label, p, m, engineOrder) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && p[i] === m[i];
  const id = x => (x === undefined ? "u" : x.id);
  if (engineOrder) console.log(label, p.length, ok ? "same" : "DIFFERENT");
  else console.log(label, p.length, id(p[0]), id(p[1]), id(p[p.length >> 1]), id(p[p.length - 1]), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelSort differs from sort: " + label);
}

const n = 40001;
const rows = [];
for (let i = 0; i < n; i++) {
  rows.push({ id: i, price: (i * 7919) % 1009, pos: { x: i % 17 === 0 ? -0 : (i * 31) % 211 } });
}
rows[5].price = Infinity;
rows[6].price = -Infinity;
rows[7].price = Infinity;
const byPrice = (a, b) => a.price - b.price;
check("ascending", rows.slice().parallelSort(byPrice), rows.slice().sort(byPrice));
const byPriceDown = (a, b) => b.price - a.price;
check("descending", rows.slice().parallelSort(byPriceDown), rows.slice().sort(byPriceDown));
const byX = (a, b) => a.pos.x - b.pos.x;
check("nested key, -0 and +0", rows.slice().parallelSort(byX), rows.slice().sort(byX));
const rank = new Float64Array(n);
for (let i = 0; i < n; i++) rank[i] = (i * 13) % 101;
const byRank = (a, b) => rank[a.id] - rank[b.id];
check("key through a captured view", rows.slice().parallelSort(byRank), rows.slice().sort(byRank));
const same = rows.slice();
console.log("returns this", same.parallelSort(byPrice) === same);
const withNaN = rows.slice(0, 5000);
withNaN[9] = { id: -1, price: NaN, pos: { x: 0 } };
check("NaN key runs sort", withNaN.slice().parallelSort(byPrice), withNaN.slice().sort(byPrice), true);
const withUndefined = rows.slice(0, 5000);
withUndefined[3] = undefined;
check("undefined element runs sort", withUndefined.slice().parallelSort(byPrice), withUndefined.slice().sort(byPrice));
const withHole = rows.slice(0, 5000);
delete withHole[4];
check("hole runs sort", withHole.slice().parallelSort(byPrice), withHole.slice().sort(byPrice));
const missing = rows.slice(0, 5000);
missing[8] = { id: -2 };
check("missing key runs sort", missing.slice().parallelSort(byPrice), missing.slice().sort(byPrice), true);
