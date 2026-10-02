// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel/sequential differential harness (docs/issues/parallel-builtins issue 02): the
// commonest callbacks that do not qualify, in a module small enough to be specialized, so the
// sequential lowering chosen beside a parallel one is the one under test. Each must answer
// what `map` does, with the same side effects in the same order. `parallel-refused.js` and
// `parallel-throws.js` cover many more, in generic modules.
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
for (let i = 0; i < 17000; i++) xs.push(i % 97);
const table = { scale: 10 };
check("property", xs.parallelMap(x => x * table.scale), xs.map(x => x * table.scale));
check("string", xs.parallelMap(x => "n" + x), xs.map(x => "n" + x));
const seen = [0, 0];
check("side-effects", xs.parallelMap((x, i) => { seen[0] = seen[0] * 31 + i >>> 0; return x; }), xs.map((x, i) => { seen[1] = seen[1] * 31 + i >>> 0; return x; }));
console.log("order", seen[0] === seen[1] ? "same" : "DIFFERENT", seen[0]);
const holey = [1, , 3, , 5];
check("holes", holey.parallelMap(x => x * 2), holey.map(x => x * 2));
const receiver = { k: 3 };
check("this-arg", xs.parallelMap(function (x) { return x * this.k; }, receiver), xs.map(function (x) { return x * this.k; }, receiver));
// An allocation the compiler removes (the array never escapes) is no allocation at all, so
// this one does run in parallel.
check("scalar-replaced", xs.parallelMap(x => [x].length + x), xs.map(x => [x].length + x));
// A callback that grows the array and then shrinks it: the length is read once, and an index
// gone by the time it is visited is a hole.
const gp = [1, 2, 3, 4];
const gm = [1, 2, 3, 4];
check("grow-shrink", gp.parallelMap((x, i) => { if (i === 0) gp.push(9); else gp.pop(); return x * 2; }), gm.map((x, i) => { if (i === 0) gm.push(9); else gm.pop(); return x * 2; }));
