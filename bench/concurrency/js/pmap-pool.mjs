// 5. Data-parallel map by hand (node, bun): a worker_threads pool, one worker per core, started
// before the clock. Each run copies its slice of the input into a Float64Array, transfers it to a
// worker, which maps it in place and transfers it back -- the copy is part of the cost, because
// it is what a pool of isolates has to pay. Results stay as per-worker Float64Arrays.
import os from "node:os";
import { Worker } from "node:worker_threads";
import { arg, emit, median, now } from "./common.mjs";

const n = Number(arg(0, 10000000));
const reps = Number(arg(1, 3));
const procs = Number(arg(2, 0)) || os.availableParallelism();
const input = [];
for (let i = 0; i < n; i += 1) input.push(i);

const workers = [];
const waiting = [];
for (let w = 0; w < procs; w += 1) {
  const worker = new Worker(new URL("./pmap-pool-worker.mjs", import.meta.url));
  worker.on("message", (value) => waiting[w](value));
  workers.push(worker);
}
function all(send) {
  return Promise.all(workers.map((worker, w) => new Promise((resolve) => {
    waiting[w] = resolve;
    send(worker, w);
  })));
}
await all((worker) => worker.postMessage(null));

let cold = 0;
let sum = 0;
const samples = [];
const chunk = Math.ceil(n / procs);
for (let r = 0; r <= reps; r += 1) {
  const started = now();
  const parts = await all((worker, w) => {
    const lo = w * chunk;
    const hi = Math.min(n, lo + chunk);
    const part = new Float64Array(Math.max(0, hi - lo));
    for (let i = lo; i < hi; i += 1) part[i - lo] = input[i];
    worker.postMessage(part, [part.buffer]);
  });
  const elapsed = now() - started;
  if (r === 0) cold = elapsed;
  else samples.push(elapsed);
  sum = 0;
  for (const part of parts) for (let i = 0; i < part.length; i += 1) sum += part[i];
}
emit({ case: "pmap-pool", cold_ms: cold, median_ms: median(samples), threads: procs, checksum: String(sum) });
await Promise.all(workers.map((worker) => worker.terminate()));
