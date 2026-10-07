// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 3 (docs/issues/parallel-builtins issue 08): `parallelFlatMap` whose callback returns arrays
// -- built in worker arenas, empty, holding objects or strings -- or plain values, over a holey
// receiver, and arrays the program already had (one level only is flattened). Each answer must be `flatMap`'s.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1500 : 30001;
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && (Object.is(p[i], m[i]) || JSON.stringify(p[i]) === JSON.stringify(m[i]));
  console.log(label, JSON.stringify(p.slice(0, 4)), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}
const xs = [];
for (let i = 0; i < n; i++) xs.push(i * 1.5);
const pairs = x => [x, x * 2];
check("pairs", xs.parallelFlatMap(pairs), xs.flatMap(pairs));
const some = (x, i) => (i & 3) === 0 ? [] : (i & 3) === 1 ? [x] : x;
check("empty, one, a value", xs.parallelFlatMap(some), xs.flatMap(some));
const objects = (x, i) => [{ at: i }, "s" + i];
check("objects and strings", xs.parallelFlatMap(objects), xs.flatMap(objects));
const nestedOnce = x => [[x], x];
check("one level only", xs.parallelFlatMap(nestedOnce), xs.flatMap(nestedOnce));
const holey = [];
for (let i = 0; i < n; i++) if (i & 3) holey[i] = i;
const around = x => [x - 1, x + 1];
check("holey receiver", holey.parallelFlatMap(around), holey.flatMap(around));
const shared = [1, 2, 3];
const reuse = (x, i) => (i & 1) === 1 ? shared : [x];
check("existing arrays", xs.parallelFlatMap(reuse), xs.flatMap(reuse));
// A hole in an answer is skipped: FlattenIntoArray advances the target only past an element it
// wrote (issue 26), whether the answer already existed or a worker built it.
const holeyShared = [1, , 3];
const reuseHoley = (x, i) => i % 2 ? holeyShared : [x];
check("existing holey arrays", xs.parallelFlatMap(reuseHoley), xs.flatMap(reuseHoley));
const builtHoley = (x, i) => (i & 3) === 0 ? [, x] : (i & 3) === 1 ? [x, , x] : [x];
check("built holey arrays", xs.parallelFlatMap(builtHoley), xs.flatMap(builtHoley));
const small = [10, 20, 30, 40];
console.log(JSON.stringify(small.parallelFlatMap(reuseHoley)), JSON.stringify(small.parallelFlatMap(builtHoley)));
