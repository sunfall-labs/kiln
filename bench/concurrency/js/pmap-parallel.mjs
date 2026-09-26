// 5. Data-parallel map with kiln's own extension (kiln only): Array.prototype.parallelMap
// (ADR 0058) over the same 10M-element array and the same inline callback as pmap.mjs.
// KILN_PARALLELISM sizes the provider's pool (default: available parallelism).
import { arg, emit, median, now } from "./common.mjs";

const n = Number(arg(0, 10000000));
const reps = Number(arg(1, 3));
const input = [];
for (let i = 0; i < n; i += 1) input.push(i);

let cold = 0;
let sum = 0;
const samples = [];
for (let r = 0; r <= reps; r += 1) {
  const started = now();
  const out = input.parallelMap((x) => {
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
emit({ case: "pmap-parallel", cold_ms: cold, median_ms: median(samples), threads: Number(process.env.KILN_PARALLELISM ?? 0) || "pool", checksum: String(sum) });
