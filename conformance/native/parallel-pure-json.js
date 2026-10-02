// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 4 (docs/issues/parallel-builtins issue 09), `JSON.stringify` of plain data on the pool:
// numbers (non-finite as null), strings with escapes, Latin1, UTF-16 and lone surrogates, nested
// literals and arrays (holes as null), class instances, and the omissions -- undefined and
// functions dropped from objects, null in arrays. What a pool thread cannot promise declines and
// runs `map`: a `toJSON` anywhere on the chain, a Date, a cycle (a TypeError), a BigInt.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1501 : 20001;
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = Object.is(p[i], m[i]);
  console.log(label, p.length, p[1], p[p.length - 1], ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}
class Point {
  constructor(x, y) { this.x = x; this.y = y; }
  norm() { return Math.hypot(this.x, this.y); }
}
class Stamped { constructor(t) { this.t = t; } toJSON() { return "T" + this.t; } }
const texts = ["plain", "quote\"back\\slash", "tab\tnl\n", "éclair", "日本", "\ud800lone", "𝒽"];
const records = [];
for (let i = 0; i < n; i++) {
  records.push({
    id: i,
    name: texts[i % texts.length],
    score: i % 9 === 0 ? Infinity : i / 7,
    tags: i % 3 === 0 ? [1, , "x"] : [],
    point: new Point(i, -i),
    skip: undefined,
    fn: () => i,
    nested: { deep: [{ ok: i % 2 === 0 }, null] },
  });
}
const plain = r => JSON.stringify(r);
check("records", records.parallelMap(plain), records.map(plain));
const parts = r => JSON.stringify([r.name, r.score, r.skip, r.fn, r.tags]) + JSON.stringify(r.skip) + JSON.stringify(r.id);
check("parts", records.parallelMap(parts), records.map(parts));
const stamped = records.map((r, i) => i === n - 2 ? { ...r, when: new Stamped(i) } : r);
check("a toJSON near the end", stamped.parallelMap(plain), stamped.map(plain));
const cyclic = records.slice();
const loop = { id: -1 };
loop.self = loop;
cyclic[n - 5] = loop;
let caught = "none";
try { cyclic.parallelMap(plain); } catch (e) { caught = e.constructor.name; }
let expected = "none";
try { cyclic.map(plain); } catch (e) { expected = e.constructor.name; }
console.log("cycle", caught, expected);
