// kiln-native: profile=native-release
//
// `map`, `filter` and `reduce` on a typed array run the call site's own loop natively
// (parallel issue 16): the callback's typed entry when the site proves which function it is,
// else a closure call per element, with the TypeScript in `kiln-builtins/src/typedarray.ts` the
// owner every failed guard falls back to. The module is kept under the release profile's
// specialization limits, so the loops here are the site's, not the generic call's.
//
// Every Number kind, answers that are doubles (typed) and answers that are not (boxed: a
// string, an object with `valueOf`, `undefined`), each written back through `ToNumber` and the
// kind's own conversion (wrapping, clamping, rounding), the index argument, and `reduce`'s
// accumulator both as a double and as a string.
const kinds = [Float64Array, Float32Array, Int32Array, Uint32Array, Int16Array, Uint16Array, Int8Array, Uint8Array, Uint8ClampedArray];
const src = [0, -0, 1.5, -1.5, 2.5, 255.5, 256, -129, 1e10, NaN, Infinity, -Infinity, 70000.7, 3];
const boxed = { valueOf: () => 41.5 };
const show = (t) => Array.prototype.map.call(t, (x) => (Object.is(x, -0) ? "-0" : String(x))).join(",");
for (const K of kinds) {
  const t = new K(src.length);
  for (let i = 0; i < src.length; i++) t[i] = src[i];
  console.log(K.name, "map", show(t.map((x) => x * 3 - 1)));
  console.log(K.name, "map boxed", show(t.map((x, i) => (i % 3 === 0 ? "7.5" : i % 3 === 1 ? boxed : undefined))));
  console.log(K.name, "filter", show(t.filter((x) => x > 1)), show(t.filter((x, i) => i & 1)));
  console.log(K.name, "reduce", t.reduce((acc, x) => acc + x, 0.5), t.reduce((acc, x) => acc + x, ""));
  console.log(K.name, "empty", t.subarray(0, 0).map((x) => x).length, t.subarray(0, 0).reduce((acc, x) => acc + x, 9));
}
