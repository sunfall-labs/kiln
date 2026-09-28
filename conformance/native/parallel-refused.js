// kiln-native: stress=skip
// The parallel/sequential differential harness (.scratch/parallel-builtins issue 02): callbacks
// that do not qualify for the pool -- they read properties, allocate, build strings, call
// functions, have side effects, read `this` or `arguments`, get a `thisArg`, or meet holes and
// non-numeric elements. Each takes the sequential lowering; `parallelMap` must still answer, and
// do, exactly what `map` does, in the same order, with the same side effects.

function show(v) {
  return Object.is(v, -0) ? "-0" : typeof v === "object" && v !== null ? JSON.stringify(v) : String(v);
}
function digest(r) {
  let h = 2166136261;
  for (let i = 0; i < r.length; i++) {
    const s = i in r ? show(r[i]) : "<hole>";
    for (let j = 0; j < s.length; j++) h = Math.imul(h ^ s.charCodeAt(j), 16777619) >>> 0;
    h = Math.imul(h ^ 44, 16777619) >>> 0;
  }
  return r.length + ":" + h.toString(16);
}
function same(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (i in a !== i in b) return false;
    if (!Object.is(a[i], b[i]) && show(a[i]) !== show(b[i])) return false;
  }
  return true;
}
function report(label, p, m) {
  const ok = same(p, m);
  console.log(label, digest(p), show(p[0]), show(p[p.length - 1]), ok ? "same" : "DIFFERENT " + digest(m));
  if (!ok) throw new Error("parallelMap differs from map: " + label);
}

const xs = [];
for (let i = 0; i < 3000; i++) xs.push(i % 97);

const table = { scale: 10 };
report("property", xs.parallelMap(x => x * table.scale), xs.map(x => x * table.scale));
const lookup = [5, 6, 7];
report("element", xs.parallelMap(x => lookup[x % 3] + x), xs.map(x => lookup[x % 3] + x));
report("allocates", xs.parallelMap(x => [x, x + 1]), xs.map(x => [x, x + 1]));
report("object", xs.parallelMap(x => ({ v: x })), xs.map(x => ({ v: x })));
report("string", xs.parallelMap(x => "n" + x), xs.map(x => "n" + x));
report("to-string", xs.parallelMap(x => x.toString(16)), xs.map(x => x.toString(16)));
report("boolean", xs.parallelMap(x => x > 40), xs.map(x => x > 40));
report("undefined", xs.parallelMap(x => { if (x > 50) return x; }), xs.map(x => { if (x > 50) return x; }));
function helper(v) {
  return v * 3;
}
report("calls", xs.parallelMap(x => helper(x) + 1), xs.map(x => helper(x) + 1));
report("math-random-free", xs.parallelMap(x => Math.max(x, Math.min(x, 3))), xs.map(x => Math.max(x, Math.min(x, 3))));
report("typeof", xs.parallelMap(x => typeof x), xs.map(x => typeof x));
report("array-arg", xs.parallelMap((x, i, a) => a.length - i), xs.map((x, i, a) => a.length - i));
report("arguments", xs.parallelMap(function (x) { return arguments.length + x; }), xs.map(function (x) { return arguments.length + x; }));
report("try", xs.parallelMap(x => { try { return x * 2; } catch (e) { return -1; } }), xs.map(x => { try { return x * 2; } catch (e) { return -1; } }));

// `this`: through a thisArg, and an arrow's captured one.
const receiver = { k: 3 };
report("this-arg", xs.parallelMap(function (x) { return x * this.k; }, receiver), xs.map(function (x) { return x * this.k; }, receiver));
report("this-arg-arrow", xs.parallelMap(x => x + 1, receiver), xs.map(x => x + 1, receiver));

// Side effects: the same calls in the same order.
const seenP = [];
const seenM = [];
const p = xs.parallelMap((x, i) => { seenP.push(i); return x; });
const m = xs.map((x, i) => { seenM.push(i); return x; });
report("side-effects", p, m);
report("side-effect-order", seenP, seenM);
let counterP = 0;
let counterM = 0;
report("counter", xs.parallelMap(x => x + counterP++), xs.map(x => x + counterM++));
console.log("counters", counterP, counterM);

// Holes and non-numeric elements: not a numeric array, so sequential, and the hole stays one.
const holey = [1, , 3, , 5];
report("holes", holey.parallelMap(x => x * 2), holey.map(x => x * 2));
const mixed = [1, "2", null, undefined, true, 4.5, NaN];
report("mixed", mixed.parallelMap(x => x * 2), mixed.map(x => x * 2));
const sparse = [];
sparse[3] = 1;
sparse[3001] = 2;
report("sparse", sparse.parallelMap(x => x + 1), sparse.map(x => x + 1));

// An array that grows while the callback runs: map visits the original length only.
const growP = [1, 2, 3];
const growM = [1, 2, 3];
report("growing", growP.parallelMap(x => { growP.push(x); return x; }), growM.map(x => { growM.push(x); return x; }));
console.log("grown", growP.length, growM.length);

// A callback held in a variable rather than written at the call.
const held = x => x * 2;
report("held", xs.parallelMap(held), xs.map(held));

// A method borrowed onto an array-like.
const like = { length: 3, 0: 1, 1: 2, 2: 3 };
report("array-like", Array.prototype.parallelMap.call(like, x => x * 5), Array.prototype.map.call(like, x => x * 5));
