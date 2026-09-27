// The node alternative: the same workloads split by hand across a worker_threads pool, one
// worker per core, the way a node user who needed the cores would write it.
//
//   node pool.mjs <workload> <n> <budget-ms> [workers]
//
// What it does, and what that favours:
//
// - The inputs live in SharedArrayBuffers, built before the clock starts (a performance-minded
//   node program would allocate them there in the first place), so no call pays to copy them to
//   the workers. Records as objects cannot be shared: each worker receives its slice once, at
//   start-up, as a structured clone, and keeps it.
// - Each call allocates its output (a fresh SharedArrayBuffer for map and sort; per-worker
//   Float64Arrays transferred back and concatenated for filter), as `map` itself allocates.
// - The pipelines are fused into one loop per worker (map, filter and sum in one pass): that is
//   what hand-written worker code does, so these rows do less work than the three-pass methods.
// - Sort: each worker sorts its run in place, then pairs of runs are merged by the workers level
//   by level; the last merge is one worker's. `sort-array` answers a Float64Array, not an array.
//
// The timing is bench/harness.mjs's, with an async workload: one call is one round trip to every
// worker and back.
import os from "node:os";
import { Worker } from "node:worker_threads";
import { bench } from "../../harness.mjs";
import { floats, ints, qtys, ratings, sortChecksum, sumChecksum, valueChecksum } from "./common.mjs";

const workload = process.argv[2];
const n = Number(process.argv[3] ?? 1000000);
const budgetMs = Number(process.argv[4] ?? 1500);
const procs = Number(process.argv[5] ?? 0) || os.availableParallelism();

function shared(values) {
  const out = new Float64Array(new SharedArrayBuffer(values.length * 8));
  out.set(values);
  return out;
}

const input = {};
if (workload === "reduce-sum" || workload === "reduce-fold") input.xs = shared(ints(n));
else if (workload === "pipeline-records") {
  input.price = shared(floats(n));
  input.qty = shared(qtys(n));
  input.rating = shared(ratings(n));
} else if (workload !== "pipeline-objects") input.xs = shared(floats(n));

// Contiguous ranges, one per worker.
const bounds = [];
for (let w = 0; w <= procs; w += 1) bounds.push(Math.floor((n * w) / procs));

const workers = [];
const waiting = [];
{
  let price, qty, rating;
  if (workload === "pipeline-objects") {
    price = floats(n);
    qty = qtys(n);
    rating = ratings(n);
  }
  for (let w = 0; w < procs; w += 1) {
    let records;
    if (workload === "pipeline-objects") {
      records = [];
      for (let i = bounds[w]; i < bounds[w + 1]; i += 1) records.push({ price: price[i], qty: qty[i], rating: rating[i] });
    }
    const worker = new Worker(new URL("./pool-worker.mjs", import.meta.url), {
      workerData: { workload, input, records, lo: bounds[w], hi: bounds[w + 1] },
    });
    worker.on("message", (value) => waiting[w](value));
    worker.on("error", (error) => {
      console.error(error);
      process.exit(1);
    });
    workers.push(worker);
  }
}

function each(count, send) {
  return Promise.all(Array.from({ length: count }, (_, w) => new Promise((resolve) => {
    waiting[w] = resolve;
    send(workers[w], w);
  })));
}

await each(procs, (worker) => worker.postMessage({ op: "ready" }));

let run;
let checksum;
if (workload === "map-light" || workload === "map-heavy") {
  run = async () => {
    const out = new Float64Array(new SharedArrayBuffer(n * 8));
    await each(procs, (worker) => worker.postMessage({ op: "map", out }));
    return out;
  };
  checksum = sumChecksum;
} else if (workload === "filter") {
  run = async () => {
    const parts = await each(procs, (worker) => worker.postMessage({ op: "filter" }));
    let total = 0;
    for (const part of parts) total += part.length;
    const out = new Float64Array(total);
    let at = 0;
    for (const part of parts) {
      out.set(part, at);
      at += part.length;
    }
    return out;
  };
  checksum = sumChecksum;
} else if (workload === "sort-typed" || workload === "sort-array") {
  run = async () => {
    let a = new Float64Array(new SharedArrayBuffer(n * 8));
    let b = new Float64Array(new SharedArrayBuffer(n * 8));
    await each(procs, (worker) => worker.postMessage({ op: "sort", a }));
    // Merge runs pairwise, level by level, one pair per worker; `runs` holds run boundaries.
    let runs = bounds.slice();
    while (runs.length > 2) {
      const next = [0];
      const jobs = [];
      for (let r = 0; r + 1 < runs.length; r += 2) {
        const lo = runs[r];
        const mid = runs[r + 1];
        const hi = r + 2 < runs.length ? runs[r + 2] : mid;
        jobs.push({ lo, mid, hi });
        next.push(hi);
      }
      const from = a;
      const to = b;
      await each(jobs.length, (worker, w) => worker.postMessage({ op: "merge", from, to, ...jobs[w] }));
      a = to;
      b = from;
      runs = next;
    }
    return a;
  };
  checksum = sortChecksum;
} else {
  run = async () => {
    const parts = await each(procs, (worker) => worker.postMessage({ op: "fold" }));
    let total = 0;
    for (const part of parts) total += part;
    return total;
  };
  checksum = valueChecksum;
}

await bench(workload, run, { budgetMs, checksum });
await Promise.all(workers.map((worker) => worker.terminate()));
