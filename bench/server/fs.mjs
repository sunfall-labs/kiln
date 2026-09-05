// Standalone filesystem benchmark suite:
// - many small reads
// - one large read
// - a stat storm
// Run under node, bun, deno, or natively compiled kiln.
//
// usage: node bench/server/fs.mjs [scenario=small-reads|large-read|stat-storm|all] [durationMs=5000] [concurrency=16]
import { readFile, stat } from "node:fs/promises";

const scenario = process.argv[2] ?? "all";
const durationMs = Number(process.argv[3] ?? 5000);
const concurrency = Number(process.argv[4] ?? 16);

const fixturesDir = "bench/server/fixtures";
const staticPath = `${fixturesDir}/static.txt`;
const largeFilePath = `${fixturesDir}/large.bin`;
const smallFilePaths = [];
for (let i = 0; i < 20; i++) {
  const pad = String(i).padStart(2, "0");
  smallFilePaths.push(`${fixturesDir}/small/f${pad}.txt`);
}
const statPaths = [];
for (let i = 0; i < 50; i++) {
  statPaths.push(smallFilePaths[i % smallFilePaths.length]);
}

async function doSmallReads() {
  let bytes = 0;
  for (let i = 0; i < smallFilePaths.length; i++) {
    const c = await readFile(smallFilePaths[i], "utf8");
    bytes += c.length;
  }
  return bytes > 0;
}

async function doLargeRead() {
  const buf = await readFile(largeFilePath);
  return buf.length > 0;
}

async function doStatStorm() {
  let total = 0;
  for (let i = 0; i < statPaths.length; i++) {
    const info = await stat(statPaths[i]);
    total += info.size;
  }
  return total > 0;
}

async function op(kind) {
  switch (kind) {
    case "small-reads":
      return await doSmallReads();
    case "large-read":
      return await doLargeRead();
    case "stat-storm":
      return await doStatStorm();
    default: {
      const a = await doSmallReads();
      const b = await doLargeRead();
      const c = await doStatStorm();
      return a && b && c;
    }
  }
}

const latencies = [];
let errors = 0;

async function worker(deadline) {
  while (Date.now() < deadline) {
    const started = process.hrtime.bigint();
    try {
      const ok = await op(scenario);
      if (!ok) errors += 1;
      const elapsedMs = Number(process.hrtime.bigint() - started) / 1e6;
      latencies.push(elapsedMs);
    } catch {
      errors += 1;
    }
  }
}

function percentile(sorted, fraction) {
  if (sorted.length === 0) return null;
  const index = Math.min(sorted.length - 1, Math.floor(sorted.length * fraction));
  return Math.round(sorted[index] * 1000) / 1000;
}

const startedAt = Date.now();
const deadline = startedAt + durationMs;
await Promise.all(Array.from({ length: concurrency }, () => worker(deadline)));
const elapsed = (Date.now() - startedAt) / 1000;

latencies.sort((a, b) => a - b);
console.log(JSON.stringify({
  scenario,
  concurrency,
  requests: latencies.length,
  errors,
  seconds: Math.round(elapsed * 1000) / 1000,
  rps: Math.round(latencies.length / elapsed),
  p50_ms: percentile(latencies, 0.5),
  p95_ms: percentile(latencies, 0.95),
  p99_ms: percentile(latencies, 0.99),
}));
