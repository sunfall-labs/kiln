// kiln-native: profile=native-release parallelism=1,2,max
//
// `parallelFlatMap` is `flatMap` (ADR 0068 rule 3; docs/issues/parallel-builtins issue 10), on
// arrays only. A pure numeric callback answers numbers, which `flatMap` never flattens, so its
// pooled lowering is `parallelMap`'s; a callback that answers arrays (or anything else) runs the
// sequential definition, which is `flatMap` itself.
function check(label, p, m) {
  let ok = p.constructor === m.constructor && p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelFlatMap differs from flatMap: " + label);
}

const n = 60000;
const xs = [];
for (let i = 0; i < n; i++) xs.push(((i * 104729) % 4099) / 7 - 250);
const bias = 0.5;
check("numeric", xs.parallelFlatMap(x => x * 2 - bias), xs.flatMap(x => x * 2 - bias));
check("index", xs.parallelFlatMap((x, i) => Math.floor(x) + i), xs.flatMap((x, i) => Math.floor(x) + i));
check("negative zero", xs.parallelFlatMap(x => -0 * x), xs.flatMap(x => -0 * x));
check("pairs", xs.parallelFlatMap(x => [x, -x]), xs.flatMap(x => [x, -x]));
check("nested", xs.slice(0, 50).parallelFlatMap(x => [[x]]).map(a => a[0]), xs.slice(0, 50).flatMap(x => [[x]]).map(a => a[0]));
const holes = [1, , 3, , 5];
check("holes", holes.parallelFlatMap(x => x * 2), holes.flatMap(x => x * 2));
check("empty", [].parallelFlatMap(x => x), [].flatMap(x => x));
const tagged = { k: 10 };
check("thisArg", xs.slice(0, 20).parallelFlatMap(function (x) { return x + this.k; }, tagged), xs.slice(0, 20).flatMap(function (x) { return x + this.k; }, tagged));
console.log(typeof new Float64Array(1).parallelFlatMap, "parallelFlatMap" in new Uint8Array(1));
