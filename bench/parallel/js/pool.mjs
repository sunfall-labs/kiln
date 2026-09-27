// The node alternative: the same workloads split by hand across a worker_threads pool, one
// worker per core, the way a node user who needed the cores would write it.
//
//   node pool.mjs <workload> <n> <budget-ms> [workers]
//
// What it does, and what that favours:
//
// - Numeric inputs live in SharedArrayBuffers, built before the clock starts (a performance-minded
//   node program would allocate them there in the first place), so no call pays to copy them to
//   the workers. Object records and CSV lines are cloned to their worker once at start-up.
// - Each call allocates its output (a fresh SharedArrayBuffer for numeric map and sort; per-worker
//   results returned and concatenated for filter and materialized rows), as `map` itself allocates.
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
import { floats, ints, qtys, ratings, recordsChecksum, sortChecksum, sumChecksum, valueChecksum } from "./common.mjs";

const workload = process.argv[2];
const n = Number(process.argv[3] ?? 1000000);
const budgetMs = Number(process.argv[4] ?? 1500);
const procs = Number(process.argv[5] ?? 0) || os.availableParallelism();
const materialized = new Set(["enrich-records", "flatmap-records", "format-csv", "parse-csv"]);

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
} else if (workload === "sort-records") {
  // Objects cannot be shared: the pool sorts their keys, as a program wanting the cores would --
  // the price column and an index permutation in shared memory -- and the main thread applies
  // the permutation to its records.
  input.price = shared(floats(n));
} else if (workload !== "pipeline-objects" && !materialized.has(workload)) input.xs = shared(floats(n));

// Contiguous ranges, one per worker.
const bounds = [];
for (let w = 0; w <= procs; w += 1) bounds.push(Math.floor((n * w) / procs));

const workers = [];
const waiting = [];
{
  let price, qty, rating, lines;
  if (workload === "pipeline-objects" || materialized.has(workload)) {
    price = floats(n);
    qty = qtys(n);
    if (workload === "pipeline-objects" || workload === "enrich-records") rating = ratings(n);
    if (workload === "parse-csv") {
      lines = [];
      for (let i = 0; i < n; i += 1) lines.push(` ${i},${qty[i]},${price[i]} `);
    }
  }
  for (let w = 0; w < procs; w += 1) {
    let records;
    if (workload === "pipeline-objects" || (materialized.has(workload) && workload !== "parse-csv")) {
      records = [];
      for (let i = bounds[w]; i < bounds[w + 1]; i += 1) {
        if (workload === "format-csv") records.push({ id: i, qty: qty[i], price: price[i] });
        else if (workload === "enrich-records" || workload === "pipeline-objects") records.push({ price: price[i], qty: qty[i], rating: rating[i] });
        else records.push({ price: price[i], qty: qty[i] });
      }
    }
    const worker = new Worker(new URL("./pool-worker.mjs", import.meta.url), {
      workerData: { workload, input, records, lines: lines?.slice(bounds[w], bounds[w + 1]), lo: bounds[w], hi: bounds[w + 1] },
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
if (materialized.has(workload)) {
  run = async () => {
    const parts = await each(procs, (worker) => worker.postMessage({ op: "materialize" }));
    return parts.flat();
  };
  if (workload === "enrich-records") {
    checksum = (out) => {
      let total = 0;
      let rated = 0;
      for (const r of out) { total += r.total; rated += r.rating; }
      return out.length + ":" + total + ":" + rated;
    };
  } else if (workload === "flatmap-records") checksum = sumChecksum;
  else if (workload === "format-csv") {
    checksum = (out) => {
      let chars = 0;
      let codes = 0;
      for (let i = 0; i < out.length; i += 1) {
        chars += out[i].length;
        if (i % 997 === 0) codes += out[i].charCodeAt(out[i].length - 1);
      }
      return out.length + ":" + chars + ":" + codes;
    };
  } else {
    checksum = (out) => {
      let qty = 0;
      let price = 0;
      for (const row of out) { qty += row[1]; price += row[2]; }
      return out.length + ":" + qty + ":" + price;
    };
  }
} else if (workload === "map-light" || workload === "map-heavy") {
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
} else if (workload === "sort-records") {
  const price = floats(n);
  const records = [];
  for (let i = 0; i < n; i += 1) records.push({ id: i, price: price[i] });
  run = async () => {
    let a = new Uint32Array(new SharedArrayBuffer(n * 4));
    let b = new Uint32Array(new SharedArrayBuffer(n * 4));
    await each(procs, (worker) => worker.postMessage({ op: "sortIndex", a }));
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
      await each(jobs.length, (worker, w) => worker.postMessage({ op: "mergeIndex", from, to, ...jobs[w] }));
      a = to;
      b = from;
      runs = next;
    }
    const out = new Array(n);
    for (let i = 0; i < n; i += 1) out[i] = records[a[i]];
    return out;
  };
  checksum = recordsChecksum;
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
