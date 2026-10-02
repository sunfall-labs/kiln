// kiln-native: profile=native-release parallelism=1,2,max
//
// `parallelSort` is `sort` (ADR 0068 rule 3; docs/issues/parallel-builtins issue 10). A stable sort
// under a consistent order has one answer, so the pool may run it: a typed array's default
// numeric order (`-0` before `+0`, `NaN` last), or a comparator proven to be `(a, b) => a - b`
// or `(a, b) => b - a` over numbers with no `NaN` -- `-0` and `+0` then compare equal and keep
// their order, which `Object.is` below sees. Any other comparator, a `NaN` under a comparator,
// and an array's default (string) order run `sort`.
function check(label, p, m) {
  let ok = p.constructor === m.constructor && p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(1), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelSort differs from sort: " + label);
}

const n = 50001;
const xs = [];
for (let i = 0; i < n; i++) xs.push(i % 5 === 0 ? (i % 2 ? -0 : 0) : ((i * 7919) % 2003) - 1001 + (i % 3) * 0.5);
xs[11] = Infinity;
xs[12] = -Infinity;
xs[13] = Infinity;
check("ascending", xs.slice().parallelSort((a, b) => a - b), xs.slice().sort((a, b) => a - b));
check("descending", xs.slice().parallelSort((a, b) => b - a), xs.slice().sort((a, b) => b - a));
const same = xs.slice();
console.log("returns this", same.parallelSort((a, b) => a - b) === same);
const withNaN = xs.slice(0, 3000);
withNaN[7] = NaN;
check("NaN runs sort", withNaN.slice().parallelSort((a, b) => a - b), withNaN.slice().sort((a, b) => a - b));
check("other comparator", xs.slice(0, 3000).parallelSort((a, b) => (a < b ? -1 : a > b ? 1 : 0)), xs.slice(0, 3000).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)));
check("array default order", xs.slice(0, 2000).parallelSort(), xs.slice(0, 2000).sort());
check("empty", [].parallelSort((a, b) => a - b), [].sort((a, b) => a - b));

// Float16's C enum slot precedes Float32, although the parallel provider does not currently
// specialize it; it must safely fall back while the neighboring Float32/Float64 kinds stay
// aligned with the shared native-kind mapping.
const kinds = [Float64Array, Float32Array, Float16Array, Int32Array, Uint32Array, Int16Array, Uint16Array, Int8Array, Uint8Array, Uint8ClampedArray];
for (let j = 0; j < kinds.length; j++) {
  const Kind = kinds[j];
  const view = new Kind(j % 3 === 0 ? 40000 : 700);
  for (let i = 0; i < view.length; i++) view[i] = xs[i];
  if (j < 2) {
    view[3] = NaN;
    view[view.length - 2] = NaN;
  }
  check(Kind.name + " default", view.slice().parallelSort(), view.slice().sort());
  if (j >= 2) check(Kind.name + " descending", view.slice().parallelSort((a, b) => b - a), view.slice().sort((a, b) => b - a));
}
const f = new Float64Array(xs);
check("Float64Array ascending", f.slice().parallelSort((a, b) => a - b), f.slice().sort((a, b) => a - b));
