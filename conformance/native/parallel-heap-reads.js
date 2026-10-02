// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 2 of the parallel built-ins (docs/issues/parallel-builtins issue 07): callbacks that read
// the heap -- own properties of shaped and dictionary objects, inherited ones through a class
// prototype and `Object.create`, absent ones, dense and holey arrays, typed arrays, and captured
// objects of every kind -- run on the pool, and must answer exactly what the sequential method
// answers. Each pair is compared element by element with `Object.is` and `in`, and a difference
// throws.
function check(label, p, m) {
  let ok = p.constructor === m.constructor && p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}

class Item {
  constructor(i) {
    this.price = (i % 97) + 0.5;
    this.qty = i % 7;
    this.currency = i % 3 === 0 ? "EUR" : i % 3 === 1 ? "USD" : "GBP";
  }
}
Item.prototype.discount = 0.25;
const n = 60001;
const items = [];
for (let i = 0; i < n; i++) items.push(new Item(i));
// A dictionary-mode record: past the shape limits, and one property deleted.
const rates = { USD: 1, EUR: 1.08, GBP: 1.27 };
for (let k = 0; k < 40; k++) rates["x" + k] = k;
delete rates.x3;
const base = { weight: 3 };
const derived = [];
for (let i = 0; i < n; i++) {
  const o = Object.create(base);
  o.id = i;
  derived.push(o);
}

const priced = x => x.price * x.qty * rates[x.currency];
check("own and dictionary", items.parallelMap(priced), items.map(priced));
const inherited = (x, i) => x.price * (1 - x.discount) + i;
check("class prototype", items.parallelMap(inherited), items.map(inherited));
const created = x => x.id * x.weight;
check("Object.create", derived.parallelMap(created), derived.map(created));
const absent = x => x.missing === undefined ? x.qty : -1;
check("absent names", items.parallelMap(absent), items.map(absent));
const eur = x => x.currency === "EUR" && x.qty > 2;
check("filter by string", items.parallelFilter(eur), items.filter(eur));
// `parallelReduce` has no sequential twin: the engines apply one fixed tree, so the answer is
// printed and compared across engines and core counts.
console.log("reduce reads", items.parallelReduce((acc, x) => acc + x.price * x.qty, (a, b) => a + b, 0));

// Holes: `map` skips them and keeps them; a read of one is undefined.
const holey = [];
for (let i = 0; i < n; i++) if (i % 5) holey[i] = i;
const scale = [2, , 4];
const holes = (x, i) => x * (scale[i % 3] === undefined ? 10 : scale[i % 3]);
check("holey receiver and read", holey.parallelMap(holes), holey.map(holes));

// Typed arrays: the receiver's elements and captured columns.
const price = new Float64Array(n);
const qty = new Int32Array(n);
const rating = new Uint8Array(n);
for (let i = 0; i < n; i++) {
  price[i] = (i % 89) + 0.25;
  qty[i] = i % 11;
  rating[i] = i % 5;
}
const score = (p, i) => Math.floor(p * qty[i] * (5 + rating[i])) + (qty[i + n] === undefined ? 1 : 0);
check("typed columns", price.parallelMap(score), price.map(score));
const radians = x => Math.sin(x * Math.PI / 180) * Math.E;
check("Math constants", price.parallelMap(radians), price.map(radians));
// A read of a number's property reaches Number.prototype: sequential.
const numberRead = x => x.length === undefined ? x : 0;
check("a number's property runs sequentially", price.parallelMap(numberRead), price.map(numberRead));
