// One worker of pool.mjs. It holds its range [lo, hi) of the shared inputs (or its own slice of
// the records) and runs the same per-element code as the sequential programs.
import { parentPort, workerData } from "node:worker_threads";

const { workload, input, records, lines, lo, hi } = workerData;
const xs = input.xs;
const cut = workload === "filter" ? 250 : workload === "pipeline-column" ? 50000 : 20000;
const fx = 1.0873;
const taxed = 1.2;

function map(out) {
  if (workload === "map-light") {
    for (let i = lo; i < hi; i += 1) out[i] = xs[i] * 1.5 + 2;
  } else {
    for (let i = lo; i < hi; i += 1) {
      const x = xs[i];
      out[i] = Math.sqrt(x * x + 1) * Math.sin(x) + Math.cos(x * 0.5);
    }
  }
}

function filter() {
  const kept = new Float64Array(hi - lo);
  let count = 0;
  for (let i = lo; i < hi; i += 1) {
    const x = xs[i];
    if (Math.sin(x) * x > cut) kept[count++] = x;
  }
  return kept.slice(0, count);
}

function fold() {
  let acc = 0;
  if (workload === "reduce-sum") {
    for (let i = lo; i < hi; i += 1) acc += xs[i];
  } else if (workload === "reduce-fold") {
    for (let i = lo; i < hi; i += 1) {
      let y = xs[i];
      for (let k = 0; k < 8; k += 1) y = (y * 31 + k) % 1000003;
      acc += y;
    }
  } else if (workload === "pipeline-column") {
    for (let i = lo; i < hi; i += 1) {
      const c = Math.floor(xs[i] * fx * taxed * 100);
      if (c > cut) acc += c;
    }
  } else if (workload === "pipeline-records") {
    const { price, qty, rating } = input;
    for (let i = lo; i < hi; i += 1) {
      const s = Math.floor(price[i] * qty[i] * (5 + rating[i]));
      if (s > cut) acc += s;
    }
  } else {
    for (const r of records) {
      const s = Math.floor(r.price * r.qty * (5 + r.rating));
      if (s > cut) acc += s;
    }
  }
  return acc;
}

function materialize() {
  if (workload === "enrich-records") {
    return records.map((r) => ({ ...r, total: r.price * r.qty }));
  }
  if (workload === "flatmap-records") {
    return records.flatMap((r) => (r.qty > 4 ? [r.price, r.qty] : []));
  }
  if (workload === "format-csv") {
    return records.map((r) => `${r.id},${r.qty},${r.price}`);
  }
  return lines.map((l) => l.trim().split(",").map(Number));
}

function merge(from, to, a, mid, b) {
  let i = a;
  let j = mid;
  let k = a;
  while (i < mid && j < b) to[k++] = from[j] < from[i] ? from[j++] : from[i++];
  while (i < mid) to[k++] = from[i++];
  while (j < b) to[k++] = from[j++];
}

// sort-records: the index permutation of [lo, hi) by `price`, stable; and a merge of two runs.
function sortIndex(a) {
  const { price } = input;
  const run = [];
  for (let i = lo; i < hi; i += 1) run.push(i);
  run.sort((x, y) => price[x] - price[y]);
  a.set(run, lo);
}

function mergeIndex(from, to, a, mid, b) {
  const { price } = input;
  let i = a;
  let j = mid;
  let k = a;
  while (i < mid && j < b) to[k++] = price[from[j]] < price[from[i]] ? from[j++] : from[i++];
  while (i < mid) to[k++] = from[i++];
  while (j < b) to[k++] = from[j++];
}

parentPort.on("message", (msg) => {
  switch (msg.op) {
    case "ready":
      return parentPort.postMessage(0);
    case "map":
      map(msg.out);
      return parentPort.postMessage(0);
    case "filter": {
      const kept = filter();
      return parentPort.postMessage(kept, [kept.buffer]);
    }
    case "materialize":
      return parentPort.postMessage(materialize());
    case "sort":
      msg.a.set(xs.subarray(lo, hi), lo);
      msg.a.subarray(lo, hi).sort();
      return parentPort.postMessage(0);
    case "merge":
      merge(msg.from, msg.to, msg.lo, msg.mid, msg.hi);
      return parentPort.postMessage(0);
    case "sortIndex":
      sortIndex(msg.a);
      return parentPort.postMessage(0);
    case "mergeIndex":
      mergeIndex(msg.from, msg.to, msg.lo, msg.mid, msg.hi);
      return parentPort.postMessage(0);
    default:
      return parentPort.postMessage(fold());
  }
});
