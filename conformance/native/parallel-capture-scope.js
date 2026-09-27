// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel/sequential differential harness (.scratch/parallel-builtins issue 02), tier 1
// (issue 05): captures that are parameters of the enclosing function, and captures threaded
// through two closures. Each is written once, so each site runs in parallel.
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

const xs = [];
for (let i = 0; i < 17000; i++) xs.push(i - 8500);
function scaled(arr, k) {
  check("parameter", arr.parallelMap(x => x * k), arr.map(x => x * k));
}
scaled(xs, 3);
scaled(xs, -0.25);
function outer(k) {
  return function inner(arr, j) {
    check("nested", arr.parallelMap((x, i) => Math.sqrt(Math.abs(x)) * k - i * j), arr.map((x, i) => Math.sqrt(Math.abs(x)) * k - i * j));
  };
}
outer(2.5)(xs, 0.5);
outer(-1)(xs, 3);
