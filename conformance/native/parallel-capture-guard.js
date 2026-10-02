// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel/sequential differential harness (docs/issues/parallel-builtins issue 02), tier 1
// (issue 05): captures the run-time guard turns away. A binding written once qualifies at
// compile time, but only a number crosses to the workers; a string, a boolean, `undefined`,
// `null`, or a binding still in its dead zone sends the call down the sequential path, which
// must answer -- or throw -- exactly as `map` does.
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
function attempt(f) {
  // The error's name: the engines word a dead-zone message differently, which is not what
  // this probe compares.
  try {
    return String(f().length);
  } catch (e) {
    return "threw " + e.name;
  }
}

const xs = [];
for (let i = 0; i < 17000; i++) xs.push(i - 8500);
const word = "a";
check("capture-string", xs.parallelMap(x => x + word), xs.map(x => x + word));
const flag = true;
check("capture-boolean", xs.parallelMap(x => x + flag), xs.map(x => x + flag));
const nothing = undefined;
check("capture-undefined", xs.parallelMap(x => x + nothing), xs.map(x => x + nothing));
const none = null;
check("capture-null", xs.parallelMap(x => x * none), xs.map(x => x * none));
function early(arr) {
  const p = attempt(() => arr.parallelMap(x => x * late));
  const m = attempt(() => arr.map(x => x * late));
  const late = 2;
  console.log("dead-zone", p, m, late);
  if (p !== m) throw new Error("parallelMap differs from map: dead-zone");
}
early(xs);
early([]);
