// Parallel issue 15: `map` against its parallel method with the pool declined, 1M elements
// (bench/parallel/fallback/run.sh). kind: "numeric" (declined at compile time: the callbacks
// read a reassigned capture), "mixed" (at run time: the array ends in a non-number), or
// "typed" (a Float64Array, declined at compile time).
const kind = process.argv[2] || "numeric";
const n = 1000000;
let bias = 1;
bias = 0;
let xs;
if (kind === "typed") xs = new Float64Array(n);
else xs = [];
for (let i = 0; i < n; i++) xs[i] = ((i * 7919) % 1000) + 0.5;
if (kind === "mixed") xs.push(null);
function time(name, f) {
  f();
  let best = Infinity;
  let r;
  for (let k = 0; k < 7; k++) {
    const t = performance.now();
    r = f();
    const d = performance.now() - t;
    if (d < best) best = d;
  }
  console.log(kind, name, best.toFixed(2), typeof r === "object" ? r.length : String(r));
}
time("map", () => xs.map((v) => v * 2 + bias));
time("par", () => xs.parallelMap((v) => v * 2 + bias));
