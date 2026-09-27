// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel/sequential differential harness (.scratch/parallel-builtins issue 02), tier 0:
// the awkward numbers -- both zeros, NaN, the infinities, denormals, the int32 and uint32
// edges, 2^53 + 1, halves -- repeated across every chunk boundary.
//
// Each file stays under `kiln-native`'s release specialization limit (16 functions, 512
// instructions), so its bodies are specialized: one `check` and at most seven pairs.
// `parallel-large-module.js` is the harness past the limit, where every body is generic and
// only the qualifying sites are lowered (issue 13).
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelMap differs from map: " + label);
}

const specials = [0, -0, NaN, Infinity, -Infinity, 1e-310, -1e-310, 2147483647, 2147483648, -2147483649, 4294967295, 4294967296, 9007199254740993, 0.5, -0.5, 1.5, 2.5, -2.5, 1e308];
const xs = [];
for (let i = 0; i < 17000; i++) xs.push(specials[i % specials.length]);
check("identity", xs.parallelMap(x => x), xs.map(x => x));
check("negate", xs.parallelMap(x => -x), xs.map(x => -x));
check("round", xs.parallelMap(x => Math.round(x) + Math.trunc(x) + Math.sign(x)), xs.map(x => Math.round(x) + Math.trunc(x) + Math.sign(x)));
check("divide", xs.parallelMap((x, i) => i / x), xs.map((x, i) => i / x));
check("remainder", xs.parallelMap(x => x % 3), xs.map(x => x % 3));
check("int32", xs.parallelMap(x => (x | 0) + (x >>> 0)), xs.map(x => (x | 0) + (x >>> 0)));
check("zero-sign", xs.parallelMap(x => x * 0 - x / Infinity), xs.map(x => x * 0 - x / Infinity));
