// Helpers every JavaScript program in bench/concurrency shares, in the surface node, bun and a
// kiln native binary built with --node all have.
//
// Each program prints one JSON line per case; run.py reads them, takes medians across
// interleaved rounds, and compares `checksum` across runtimes. A case whose checksum differs from
// the reference did different work and is excluded from the table, never reported as a time.
import fs from "node:fs";

export function now() {
  return performance.now();
}

export function emit(fields) {
  console.log(JSON.stringify(fields));
}

export function median(values) {
  const sorted = values.slice().sort((a, b) => a - b);
  return sorted[sorted.length >> 1];
}

/// Resident set of this process in KiB, from /proc (Linux). -1 where it cannot be read.
export function rssKiB() {
  try {
    const status = fs.readFileSync("/proc/self/status", "utf8");
    const match = /VmRSS:\s+(\d+)/.exec(status);
    return match ? Number(match[1]) : -1;
  } catch {
    return -1;
  }
}

export function arg(index, fallback) {
  const value = process.argv[2 + index];
  return value === undefined ? fallback : value;
}

/// Time `op` (sync or async) in batches until `budgetMs` has passed; median milliseconds per op.
/// The first batch is a warm-up and is not a sample.
export async function perOp(budgetMs, batch, op) {
  for (let i = 0; i < batch; i += 1) await op();
  const samples = [];
  const deadline = now() + budgetMs;
  while (now() < deadline || samples.length < 3) {
    const started = now();
    for (let i = 0; i < batch; i += 1) await op();
    samples.push((now() - started) / batch);
  }
  return median(samples);
}

// The shared workload pieces, identical in every program and in go/main.go.

export function smallMessage(i) {
  return { id: i, name: "order", tags: ["a", "b"], qty: 3, price: 9.5 };
}

/// 6000 items, each pointing at one of 13 shared category objects: a graph, not a tree, about
/// 1 MB once structured-cloned.
export function buildGraph() {
  const cats = [];
  for (let i = 0; i < 13; i += 1) cats.push({ code: i * 3, label: "category-" + i });
  const items = [];
  for (let i = 0; i < 6000; i += 1) {
    items.push({ id: i, name: "item-" + i, tags: ["t" + (i % 13), "group-" + (i % 97)], price: (i % 1000) / 4, cat: cats[i % 13] });
  }
  return items;
}

export function walkGraph(items) {
  let sum = 0;
  for (const it of items) sum += it.id + it.name.length + it.tags[1].length + Math.trunc(it.price * 4) + it.cat.code;
  return sum;
}

export function fillBuffer(bytes) {
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = (i * 7 + (i >> 12)) & 255;
}

export function touchBuffer(bytes) {
  let sum = 0;
  for (let i = 0; i < bytes.length; i += 4096) sum += bytes[i];
  return sum;
}

export const TABLE_LEN = (50 * 1024 * 1024) / 8;

export function fillTable(table) {
  // (i * 2654435761) % 1000003 without leaving exact integer range: split the multiplier.
  for (let i = 0; i < table.length; i += 1) {
    table[i] = ((i % 1000003) * (2654435761 % 1000003)) % 1000003;
  }
}

export function lookups(table, seed, queries) {
  let x = seed >>> 0;
  let sum = 0;
  const n = table.length;
  for (let i = 0; i < queries; i += 1) {
    x = (Math.imul(x, 1664525) + 1013904223) >>> 0;
    sum += table[x % n];
  }
  return sum;
}

export function mapOne(x) {
  let y = x;
  for (let k = 0; k < 16; k += 1) y = (y * 31 + k) % 1000003;
  return y;
}
