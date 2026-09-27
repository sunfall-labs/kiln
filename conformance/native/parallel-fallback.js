// kiln-native: profile=native-release parallelism=1,max
//
// A parallel method whose pool declines runs its sequential method's own loop at the site
// (parallel issue 15): `parallelMap`, `parallelFilter` and `parallelSort` are `map`, `filter`
// and `sort` there. Declined at compile time (the callbacks read a reassigned capture) over an
// array and a `Float64Array`, and at run time over an array holding non-numbers. The module is
// kept under the release profile's specialization limits, so these are the site's loops.
let bias = 1;
bias = 0;
const xs = [];
for (let i = 0; i < 3000; i++) xs.push(((i * 7919) % 1000) - 500.5);
const same = (a, b) => a.length === b.length && Array.prototype.every.call(a, (x, i) => Object.is(x, b[i]));
for (const [label, v] of [["array", xs], ["mixed", xs.concat([null, "7"])], ["typed", new Float64Array(xs)]]) {
  console.log(label, "map", same(v.parallelMap((x) => x * 2 + bias), v.map((x) => x * 2 + bias)));
  console.log(label, "filter", same(v.parallelFilter((x) => x > bias), v.filter((x) => x > bias)));
  console.log(label, "sort", same(v.slice().parallelSort((a, b) => a - b + bias), v.slice().sort((a, b) => a - b + bias)));
}
