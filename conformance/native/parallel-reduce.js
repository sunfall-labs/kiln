// kiln-native: profile=native-release parallelism=1,2,max
//
// `parallelReduce(callback, combine, initial)` has no sequential equivalent, so its meaning is
// written once (ADR 0069; `kiln-builtins/src/parallel.ts`): leaves of 256 indices, each folded
// left to right from `initial`, then combined pairwise, level by level, an odd last one carried
// up. The tree depends on the length alone, so a combine that is not associative -- a
// floating-point sum -- gives the same bits in both engines at every core count. `tree` below is
// that definition written out again; every answer is held to it with `Object.is`, and where the
// combine is exact (integers) to plain `reduce` as well.
function tree(xs, fn, combine, initial) {
  let level = [];
  for (let s = 0; s < xs.length; s += 256) {
    let acc = initial;
    for (let i = s; i < Math.min(s + 256, xs.length); i++) if (i in xs) acc = fn(acc, xs[i], i, xs);
    level.push(acc);
  }
  if (level.length === 0) return initial;
  while (level.length > 1) {
    const next = [];
    let i = 0;
    for (; i + 1 < level.length; i += 2) next.push(combine(level[i], level[i + 1]));
    if (i < level.length) next.push(level[i]);
    level = next;
  }
  return level[0];
}
function check(label, p, t) {
  const ok = Object.is(p, t);
  console.log(label, Object.is(p, -0) ? "-0" : String(p), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelReduce differs from its tree: " + label);
}

const xs = [];
for (let i = 0; i < 100003; i++) xs.push(Math.sin(i) * 1000 + (i % 7) * 0.1);
const add = (a, b) => a + b;
const scale = 0.001;
check("float sum", xs.parallelReduce((a, x) => a + x, add, 0), tree(xs, (a, x) => a + x, add, 0));
check("capture and index", xs.parallelReduce((a, x, i) => a + x * scale + i, add, 0), tree(xs, (a, x, i) => a + x * scale + i, add, 0));
check("max", xs.parallelReduce((a, x) => Math.max(a, x), (a, b) => Math.max(a, b), -Infinity), tree(xs, (a, x) => Math.max(a, x), (a, b) => Math.max(a, b), -Infinity));
check("non-associative", xs.parallelReduce((a, x) => a * 0.5 + x, (a, b) => a - b * 0.25, 1), tree(xs, (a, x) => a * 0.5 + x, (a, b) => a - b * 0.25, 1));
const ints = [];
for (let i = 0; i < 70000; i++) ints.push((i * 7919) % 1000);
check("integer sum", ints.parallelReduce((a, x) => a + x, add, 0), ints.reduce((a, x) => a + x, 0));
for (const n of [0, 1, 255, 256, 257, 512, 513, 769]) {
  const part = xs.slice(0, n);
  check("length " + n, part.parallelReduce((a, x) => a + x, add, 0), tree(part, (a, x) => a + x, add, 0));
}
check("NaN initial", xs.parallelReduce((a, x) => a + x, add, NaN), tree(xs, (a, x) => a + x, add, NaN));
check("-0", [-0, -0].parallelReduce((a, x) => a + x, add, -0), tree([-0, -0], (a, x) => a + x, add, -0));

const kinds = [Float64Array, Float32Array, Int32Array, Uint8Array, Int16Array];
for (let j = 0; j < kinds.length; j++) {
  const view = new kinds[j](j === 0 ? 50000 : 3000);
  for (let i = 0; i < view.length; i++) view[i] = xs[i];
  check(kinds[j].name, view.parallelReduce((a, x) => a + x * 2, add, 0), tree(view, (a, x) => a + x * 2, add, 0));
}

// Sequential forms, same tree: a string initial, a combine that builds strings, a hole.
const sp = ints.slice(0, 600).parallelReduce((a, x) => a + x, add, "");
const st = tree(ints.slice(0, 600), (a, x) => a + x, add, "");
check("string initial", sp === st ? sp.length : -1, st.length);
const holes = [1, 2, , 4];
check("holes", holes.parallelReduce((a, x) => a + x, add, 0), tree(holes, (a, x) => a + x, add, 0));
