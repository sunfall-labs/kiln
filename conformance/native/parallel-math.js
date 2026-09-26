// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel/sequential differential harness (.scratch/parallel-builtins issue 02), tier 0:
// the listed `Math` functions, which the parallel clone lowers to bare libm calls, against the
// same calls in `map`; and the smallest arrays, which the pool runs inline.
//
// A release module past 16 functions or 512 instructions is emitted with generic bodies only
// (`kiln-native`'s specialization limit), which has no parallel lowering at all, so each file
// in this harness stays under it: one `check` and at most seven pairs.
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelMap differs from map: " + label);
}

const xs = [];
for (let i = 0; i < 17000; i++) xs.push(i * 0.75 - 6000);
check("sqrt", xs.parallelMap(x => Math.sqrt(Math.abs(x)) + Math.floor(x / 3) - Math.ceil(x / 7)), xs.map(x => Math.sqrt(Math.abs(x)) + Math.floor(x / 3) - Math.ceil(x / 7)));
check("trig", xs.parallelMap(x => Math.sin(x) * Math.cos(x) + Math.tan(x / 9000) + Math.atan2(x, 3) - Math.hypot(x, 4)), xs.map(x => Math.sin(x) * Math.cos(x) + Math.tan(x / 9000) + Math.atan2(x, 3) - Math.hypot(x, 4)));
check("exp-log", xs.parallelMap(x => Math.log1p(Math.abs(x)) + Math.exp(-Math.abs(x) / 1000) + Math.cbrt(x) + Math.tanh(x / 100)), xs.map(x => Math.log1p(Math.abs(x)) + Math.exp(-Math.abs(x) / 1000) + Math.cbrt(x) + Math.tanh(x / 100)));
check("min-max", xs.parallelMap((x, i) => Math.min(-i, x) + Math.max(i, x / 2) + Math.pow(x / 6000, 2)), xs.map((x, i) => Math.min(-i, x) + Math.max(i, x / 2) + Math.pow(x / 6000, 2)));
check("rounding", xs.parallelMap(x => Math.round(x / 4) + Math.trunc(x / 3) + Math.sign(x)), xs.map(x => Math.round(x / 4) + Math.trunc(x / 3) + Math.sign(x)));
check("small", [3, -1, 0.5].parallelMap(x => x * x), [3, -1, 0.5].map(x => x * x));
check("empty", [].parallelMap(x => x + 1), [].map(x => x + 1));
