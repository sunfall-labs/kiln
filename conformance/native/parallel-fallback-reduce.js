// kiln-native: profile=native-release parallelism=1,max
//
// `parallelReduce` whose pool declines runs its tree sequentially at the site (parallel issue
// 15) -- leaves of 256 folded from `initial`, then combined pairwise level by level, as
// `kiln-builtins/src/parallel.ts` applies it -- typed when both callbacks' typed entries answer
// numbers, else through closure calls. Declined at compile time (a reassigned capture) over an
// array and a `Float64Array`, and at run time over an array holding non-numbers and for
// callbacks that answer objects, which the tree's levels must keep alive.
let bias = 1;
bias = 0;
const xs = [];
for (let i = 0; i < 3000; i++) xs.push(((i * 7919) % 1000) - 500.5);
for (const [label, v] of [["array", xs], ["mixed", xs.concat([null, "7"])], ["typed", new Float64Array(xs)]]) {
  console.log(label, "reduce", v.parallelReduce((a, x) => a + x + bias, (a, b) => a + b, 0), v.reduce((a, x) => a + x + bias, 0));
  const tree = v.parallelReduce((a, x, i) => ({ n: a.n + 1, last: i }), (a, b) => ({ n: a.n + b.n, last: b.last }), { n: 0, last: -1 });
  console.log(label, "objects", tree.n, tree.last);
}
const sum = (a, b) => a + b;
console.log("empty", [].parallelReduce(sum, sum, "init"), new Float64Array(0).parallelReduce(sum, sum, 4), [5].parallelReduce(sum, sum, 1));
