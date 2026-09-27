// 3. A large read-only dataset shared by N workers (node, bun).
//
// The dataset is a 50 MB Float64Array lookup table; every worker runs the same number of
// pseudo-random lookups over it. JavaScript cannot share an object graph between threads, so the
// two options are the two rows:
//
//   copy  -- postMessage(table.buffer) to each worker: N structured-clone copies.
//   sab   -- the table lives in a SharedArrayBuffer every worker maps (bytes only).
//
// setup_ms is from the first send until every worker holds the table; query_ms is the median of
// three rounds from "go" to every worker's answer.
import { Worker } from "node:worker_threads";
import { TABLE_LEN, arg, emit, fillTable, median, now, rssKiB } from "./common.mjs";

const n = Number(arg(0, 4));
const queries = Number(arg(1, 2000000));
const mode = arg(2, "copy");

let started = now();
const backing = mode === "sab" ? new SharedArrayBuffer(TABLE_LEN * 8) : new ArrayBuffer(TABLE_LEN * 8);
const table = new Float64Array(backing);
fillTable(table);
const build = now() - started;

const workers = [];
const waiting = [];
for (let w = 0; w < n; w += 1) {
  const worker = new Worker(new URL("./shared-worker.mjs", import.meta.url));
  worker.on("message", (value) => waiting[w](value));
  workers.push(worker);
}
function all(send) {
  return Promise.all(workers.map((worker, w) => new Promise((resolve) => {
    waiting[w] = resolve;
    send(worker, w);
  })));
}
// Workers are started before the clock so setup measures distributing the table, not booting.
await all((worker) => worker.postMessage({ kind: "ping" }));

started = now();
await all((worker) => worker.postMessage({ kind: "table", buffer: backing }));
const setup = now() - started;
const rss = rssKiB();

const samples = [];
let total = 0;
for (let round = 0; round < 3; round += 1) {
  started = now();
  const results = await all((worker, w) => worker.postMessage({ kind: "query", seed: w + 1, queries }));
  samples.push(now() - started);
  total = results.reduce((a, b) => a + b, 0);
}
emit({ case: `shared-${n}-${mode}`, build_ms: build, setup_ms: setup, query_ms: median(samples), rss_kib: rss, checksum: String(total) });
await Promise.all(workers.map((worker) => worker.terminate()));
