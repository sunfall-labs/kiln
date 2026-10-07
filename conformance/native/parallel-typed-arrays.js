// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel/sequential differential harness (docs/issues/parallel-builtins issue 02): typed
// arrays of every kind. The pooled lowering reads and writes element storage directly, so each
// kind's conversion -- wrapping for the integers, rounding for Float32, clamping with
// round-half-to-even for Uint8Clamped -- must be exactly the one `map` applies when it stores
// into its result. Qualifying callbacks, the index parameter and a tier 1 capture, over values
// on every conversion edge.
//
// Each file stays under `kiln-native`'s release specialization limit (16 functions, 512
// instructions), so its bodies are specialized: one `check` and at most seven pairs.
// `parallel-large-module.js` is the harness past the limit, where every body is generic and
// only the qualifying sites are lowered (issue 13).
function check(label, p, m) {
  let ok = p.constructor === m.constructor && p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelMap differs from map: " + label);
}

const edges = [0, -0, 0.5, 1.5, 2.5, -0.5, -1.5, 127, 128, -128, -129, 254.5, 255, 255.5, 256, 300, -1, 32767, 32768, -32769, 65535, 65536, 2147483647, 2147483648, -2147483649, 4294967295, 4294967296, 1e10, NaN, Infinity, -Infinity, 1e-40, 3.4028235677973366e38, 16777217];
const factor = 3;
// Some kinds at a size the pool splits across threads (its floor is 16,384 elements); below
// it the same chunk worker runs inline, so every kind's conversion is exercised either way.
const kinds = [Float64Array, Float32Array, Int32Array, Uint32Array, Int16Array, Uint16Array, Int8Array, Uint8Array, Uint8ClampedArray];
for (let k = 0; k < kinds.length; k++) {
  const Kind = kinds[k];
  const n = k % 4 === 0 ? 17000 : 400;
  const src = new Kind(n);
  for (let i = 0; i < n; i++) src[i] = edges[i % edges.length] + (i % 5) * 0.25;
  const name = Kind.name;
  check(name + " affine", src.parallelMap(x => x * 2.5 - 7), src.map(x => x * 2.5 - 7));
  check(name + " edges", src.parallelMap((x, i) => (i % 2 === 0 ? x + 0.5 : -x - 0.5)), src.map((x, i) => (i % 2 === 0 ? x + 0.5 : -x - 0.5)));
  check(name + " index", src.parallelMap((x, i) => x + i), src.map((x, i) => x + i));
  check(name + " capture", src.parallelMap(x => x * factor + 0.5), src.map(x => x * factor + 0.5));
  check(name + " math", src.parallelMap(x => Math.round(x / 3) + Math.sqrt(Math.abs(x))), src.map(x => Math.round(x / 3) + Math.sqrt(Math.abs(x))));
  check(name + " empty", new Kind(0).parallelMap(x => x + 1), new Kind(0).map(x => x + 1));
}
