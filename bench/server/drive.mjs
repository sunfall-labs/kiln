// The load client. Always runs under node, whatever is serving, so the runtime under test is the
// only thing that differs between rows. Keep-alive is on: a per-request connection would measure
// the kernel's accept path more than the server's, and every runtime here supports reuse.
//
// Latency is recorded per request and reported as percentiles rather than a mean, because a mean
// hides exactly the tail a server is judged on.
import http from "node:http";

const port = 18088;
const durationMs = Number(process.argv[2] ?? 5000);
const concurrency = Number(process.argv[3] ?? 16);

const agent = new http.Agent({ keepAlive: true, maxSockets: concurrency });
const latencies = [];
let errors = 0;

function once() {
  return new Promise((resolve) => {
    const started = process.hrtime.bigint();
    const request = http.request(
      { host: "127.0.0.1", port, path: "/", agent },
      (response) => {
        response.resume();
        response.on("end", () => {
          latencies.push(Number(process.hrtime.bigint() - started) / 1e6);
          resolve(true);
        });
      },
    );
    request.on("error", () => resolve(false));
    request.end();
  });
}

async function worker(deadline) {
  while (Date.now() < deadline) {
    if (!(await once())) errors += 1;
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
  requests: latencies.length,
  errors,
  seconds: Math.round(elapsed * 1000) / 1000,
  rps: Math.round(latencies.length / elapsed),
  p50_ms: percentile(latencies, 0.5),
  p95_ms: percentile(latencies, 0.95),
  p99_ms: percentile(latencies, 0.99),
}));
