// 1. Worker start cost (node, bun): start a worker_threads Worker and wait for its first message.
//
//   start       -- one cold start, then the median of warm starts (each worker is terminated
//                  before the next starts, so this is start cost, not pool reuse).
//   mem <N>     -- start N workers, wait until each has replied, read this process's RSS.
import { Worker } from "node:worker_threads";
import { arg, emit, median, now, rssKiB } from "./common.mjs";

const workerUrl = new URL("./start-worker.mjs", import.meta.url);

function startOne(i) {
  return new Promise((resolve, reject) => {
    const worker = new Worker(workerUrl, { workerData: i });
    worker.once("message", (value) => resolve({ worker, value }));
    worker.once("error", reject);
  });
}

const mode = arg(0, "start");
if (mode === "start") {
  const warm = Number(arg(1, 20));
  let sum = 0;
  let started = now();
  let { worker, value } = await startOne(0);
  const cold = now() - started;
  sum += value;
  await worker.terminate();
  const samples = [];
  for (let i = 1; i <= warm; i += 1) {
    started = now();
    ({ worker, value } = await startOne(i));
    samples.push(now() - started);
    sum += value;
    await worker.terminate();
  }
  emit({ case: "start", cold_ms: cold, median_ms: median(samples), checksum: String(sum) });
} else {
  const n = Number(arg(1, 1));
  const before = rssKiB();
  const started = await Promise.all(Array.from({ length: n }, (_, i) => startOne(i)));
  // Let each worker finish settling (its first message is sent from the module body).
  await new Promise((resolve) => setTimeout(resolve, 200));
  const after = rssKiB();
  let sum = 0;
  for (const { value } of started) sum += value;
  emit({ case: `mem-${n}`, rss_before_kib: before, rss_kib: after, per_worker_kib: (after - before) / n, checksum: String(sum) });
  await Promise.all(started.map(({ worker }) => worker.terminate()));
}
