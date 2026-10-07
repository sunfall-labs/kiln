// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 3 of the parallel built-ins (docs/issues/parallel-builtins issue 08): callbacks that build
// objects -- literals, `{ ...r, total }`, members that overwrite a spread one, values whose kind
// varies (a slot's double bit is part of its shape), computed keys, nested literals and arrays --
// run on the pool in worker arenas and must answer exactly what `map` answers: the same values
// under the same keys in the same order. Each element is compared by its own keys (in order) and
// by JSON; a difference throws. The size shrinks under `KILN_GC_STRESS`, and nothing printed
// depends on it.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1500 : 30001;
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) {
    const a = p[i], b = m[i];
    ok = typeof a === typeof b && Object.keys(a).join(",") === Object.keys(b).join(",") &&
      JSON.stringify(a) === JSON.stringify(b);
  }
  console.log(label, JSON.stringify(p[0]), JSON.stringify(p[7]), Object.keys(p[3]).join(","), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}

const kinds = ["apple", "pear", "fig"];
const rows = [];
for (let i = 0; i < n; i++) {
  rows.push({ id: i, qty: i % 7, price: (i % 97) + 0.25, kind: kinds[i % 3] });
}

const enrich = r => ({ ...r, total: r.qty * r.price });
check("enrich", rows.parallelMap(enrich), rows.map(enrich));
const literal = r => ({ id: r.id, name: "n" + r.id, big: r.qty > 3, none: null });
check("literal", rows.parallelMap(literal), rows.map(literal));
const overwrite = r => ({ ...r, qty: r.qty + 1, extra: r.id % 5 });
check("overwrite a spread member", rows.parallelMap(overwrite), rows.map(overwrite));
const varying = r => ({ v: r.id % 2 ? r.id : "s" + r.id, w: r.id % 3 ? undefined : r.price });
check("value kinds vary per element", rows.parallelMap(varying), rows.map(varying));
const nested = (r, i) => ({ id: r.id, pair: [r.qty, r.price], tag: { k: r.kind, at: i } });
check("nested literals", rows.parallelMap(nested), rows.map(nested));
const computed = r => ({ [r.kind]: r.id, after: 1 });
check("computed keys", rows.parallelMap(computed), rows.map(computed));
const maybe = r => ({ ...(r.id % 4 === 0 ? null : r), seen: true });
check("spread of null", rows.parallelMap(maybe), rows.map(maybe));
const same = r => r.id % 2 ? r : { id: r.id };
check("answers that already exist", rows.parallelMap(same), rows.map(same));

// A second pass over the enriched rows: their shapes now exist, and a spread of a spread is the
// same object again.
const again = rows.parallelMap(enrich);
const twice = r => ({ ...r, total: r.total * 2 });
check("spread of built rows", again.parallelMap(twice), again.map(twice));

// A getter on one row near the end: the pool declines there and the call runs `map` from the
// first element -- every getter call happens once, in order.
const log = [];
const odd = rows.slice();
const tricky = { id: -1, qty: 2, kind: "fig" };
Object.defineProperty(tricky, "price", { get() { log.push("get"); return 4; }, enumerable: true });
odd[n - 3] = tricky;
const priced = r => ({ id: r.id, cost: r.qty * r.price });
const p = odd.parallelMap(priced);
const got = log.length;
log.length = 0;
const m = odd.map(priced);
check("a getter declines the pool", p, m);
console.log("getter calls", got, log.length);
