// kiln-native: profile=native-release parallelism=1,2,max
//
// `parallelFilter` is `filter` (ADR 0068 rule 3; docs/issues/parallel-builtins issue 10). A pure
// numeric predicate -- one returning booleans, or numbers read through `ToBoolean` -- runs on the
// pool as a keep mask, and the kept elements are copied in index order; every other call runs the
// sequential definition in `kiln-builtins/src/parallel.ts`, which is `filter` itself. Each pair
// is compared element by element with `Object.is`, and a difference throws.
function check(label, p, m) {
  let ok = p.constructor === m.constructor && p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelFilter differs from filter: " + label);
}

const n = 70001;
const xs = [];
for (let i = 0; i < n; i++) xs.push(((i * 7919) % 2003) - 1001 + (i % 4) * 0.25);
xs[5] = -0;
xs[6] = NaN;
xs[7] = Infinity;
const k = 3;
check("array boolean", xs.parallelFilter(x => x > 10), xs.filter(x => x > 10));
check("array numeric truthiness", xs.parallelFilter(x => x % k), xs.filter(x => x % k));
check("array index", xs.parallelFilter((x, i) => (x + i) % 2 === 0 || i < 3), xs.filter((x, i) => (x + i) % 2 === 0 || i < 3));
check("array NaN and -0 answers", xs.parallelFilter(x => (x > 0 ? NaN : x * 0)), xs.filter(x => (x > 0 ? NaN : x * 0)));
check("array none", xs.parallelFilter(x => x !== x && x === x), xs.filter(x => x !== x && x === x));
check("array empty", [].parallelFilter(x => x > 0), [].filter(x => x > 0));

const kinds = [Float64Array, Float32Array, Int32Array, Uint32Array, Int16Array, Uint16Array, Int8Array, Uint8Array, Uint8ClampedArray];
for (let j = 0; j < kinds.length; j++) {
  const Kind = kinds[j];
  const view = new Kind(j % 3 === 0 ? 40000 : 500);
  for (let i = 0; i < view.length; i++) view[i] = xs[i];
  check(Kind.name + " boolean", view.parallelFilter(x => x >= 0), view.filter(x => x >= 0));
  check(Kind.name + " capture", view.parallelFilter((x, i) => x * k > i % 97), view.filter((x, i) => x * k > i % 97));
}
