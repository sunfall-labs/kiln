// The element-storage primitives under `%TypedArray%.prototype` (ADR 0065, parallel issue 16):
// `__kilnTypedArraySort` -- the default order, `-0` before `+0` and every `NaN` last, owned by
// `kiln_number_core::typed_order` for both engines -- and `__kilnTypedArrayCopy`, the same-type
// byte copy under `slice`, `set`, `copyWithin`, `fill` and `toSorted`. Overlapping copies read as
// if through a temporary; two element types take the element loop. (A comparator is consistent
// only without `NaN`, so the comparator sort drops them first: otherwise the order is the
// engine's.)
// Keep every native kind represented here: in particular Float16 sits between Uint32 and
// Float32 in the C enum, while BigInt64 sorts signed rather than as its unsigned bit pattern.
const kinds = [Float64Array, Float32Array, Float16Array, Int32Array, Uint32Array, Int16Array, Uint16Array, Int8Array, Uint8Array, Uint8ClampedArray];
const src = [3, -0, NaN, 0, -7.5, Infinity, 1e-300, -Infinity, 2, NaN, -0, 129, -129, 65536.5];
const show = (t) => Array.prototype.map.call(t, (x) => (Object.is(x, -0) ? "-0" : String(x))).join(",");
for (const K of kinds) {
  const t = new K(src.length);
  for (let i = 0; i < src.length; i++) t[i] = src[i];
  const sorted = t.toSorted();
  console.log(K.name, "toSorted", show(sorted), "source", show(t));
  console.log(K.name, "sort", show(t.slice().sort()), "comparator", show(t.filter((x) => x === x).sort((a, b) => b - a)));
  console.log(K.name, "slice", show(t.slice(2, -3)), show(t.slice(-2)), t.slice(5, 1).length);
  const c = t.slice();
  c.copyWithin(2, 0, 6);
  const d = t.slice();
  d.copyWithin(0, 3);
  console.log(K.name, "copyWithin", show(c), show(d));
  const s = t.slice();
  s.set(s.subarray(0, 5), 3);
  const u = t.slice();
  u.set(new Float64Array([1.5, -2.5, 300]), 1);
  console.log(K.name, "set", show(s), show(u));
  console.log(K.name, "fill", show(new K(9).fill(-1.5, 1, 8)), show(new K(5).fill(2)));
  console.log(K.name, "join", t.join(), t.join(" | "));
}
const bi = new BigInt64Array([5n, -9223372036854775808n, 0n, 9223372036854775807n, -1n]);
const bu = new BigUint64Array([5n, 18446744073709551615n, 0n, 7n]);
console.log("bigint", bi.slice().sort().join(","), bu.toSorted().join(","), bi.slice(1, 3).join(","));
console.log("__kilnBufferTypedArrayCopy" in globalThis);
