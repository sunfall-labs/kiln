// 5. Data-parallel map, sequential baseline (node, bun, kiln): Array.prototype.map over a
// 10M-element array of numbers. The callback is written inline, identically in every program, so
// a compiler that can prove it pure may parallelise it (kiln's parallelMap does; see
// pmap-parallel.mjs). One cold run, then the median of `reps` warm runs.
import { arg, emit, median, now } from "./common.mjs";

const n = Number(arg(0, 10000000));
const reps = Number(arg(1, 3));
// "light": a callback small enough for kiln's typed parallelMap pool to accept, (x * 3 + 1) %
// 1000003 -- memory-bound rather than compute-bound, so it measures the machinery, not the cores.
const light = arg(2, "heavy") === "light";
const input = [];
for (let i = 0; i < n; i += 1) input.push(i);

let cold = 0;
let sum = 0;
const samples = [];
for (let r = 0; r <= reps; r += 1) {
  const started = now();
  const out = light
    ? input.map((x) => (x * 3 + 1) % 1000003)
    : input.map((x) => {
      let y = x;
      for (let k = 0; k < 16; k += 1) y = (y * 31 + k) % 1000003;
      return y;
    });
  const elapsed = now() - started;
  if (r === 0) cold = elapsed;
  else samples.push(elapsed);
  sum = 0;
  for (let i = 0; i < out.length; i += 1) sum += out[i];
}
emit({ case: light ? "pmap-light-map" : "pmap-map", cold_ms: cold, median_ms: median(samples), threads: 1, checksum: String(sum) });
