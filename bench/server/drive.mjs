// The load client. Always runs under node, whatever is serving, so the runtime under test is the
// only thing that differs between rows.
//
// Supports scenarios:
// - hello (GET /)
// - static / static-file (GET /static)
// - json-1k / json-echo-1k (POST /echo with 1 KiB JSON body)
// - json-64k / json-echo-64k (POST /echo with 64 KiB JSON body)
// - stream / chunked-stream (GET /stream)
// - upload / large-upload (POST /upload with 256 KiB body)
// - churn / connection-churn (GET /churn with keepAlive: false)
// - fs-small-reads (GET /fs/small-reads)
// - fs-large-read (GET /fs/large-read)
// - fs-stat-storm (GET /fs/stat-storm)
//
// usage: node drive.mjs [durationMs=5000] [concurrency=16] [port=18088] [scenario=hello]
import http from "node:http";

const durationMs = Number(process.argv[2] ?? 5000);
const concurrency = Number(process.argv[3] ?? 16);
const port = Number(process.argv[4] ?? 18088);
const scenario = String(process.argv[5] ?? "hello");
if (![durationMs, concurrency, port].every(Number.isSafeInteger) || durationMs <= 0 || concurrency <= 0 || port <= 0 || port > 65535) {
  throw new Error("duration, concurrency, and port must be positive integers");
}

const isChurn = scenario === "churn" || scenario === "connection-churn";
const agent = new http.Agent({ keepAlive: !isChurn, maxSockets: concurrency });

let method = "GET";
let path = "/";
let bodyData = null;
const headers = {};

switch (scenario) {
  case "static":
  case "static-file":
    path = "/static";
    break;
  case "json-1k":
  case "json-echo-1k":
    method = "POST";
    path = "/echo";
    bodyData = JSON.stringify({ kind: "1k", data: "x".repeat(950), nums: [1, 2, 3, 4, 5] });
    headers["content-type"] = "application/json";
    headers["content-length"] = String(Buffer.byteLength(bodyData));
    break;
  case "json-64k":
  case "json-echo-64k":
    method = "POST";
    path = "/echo";
    bodyData = JSON.stringify({ kind: "64k", data: "x".repeat(65000), nums: [1, 2, 3, 4, 5] });
    headers["content-type"] = "application/json";
    headers["content-length"] = String(Buffer.byteLength(bodyData));
    break;
  case "stream":
  case "chunked-stream":
    path = "/stream";
    break;
  case "upload":
  case "large-upload":
    method = "POST";
    path = "/upload";
    bodyData = "u".repeat(262144); // 256 KiB
    headers["content-type"] = "application/octet-stream";
    headers["content-length"] = String(Buffer.byteLength(bodyData));
    break;
  case "churn":
  case "connection-churn":
    path = "/churn";
    headers["connection"] = "close";
    break;
  case "fs-small-reads":
    path = "/fs/small-reads";
    break;
  case "fs-large-read":
    path = "/fs/large-read";
    break;
  case "fs-stat-storm":
    path = "/fs/stat-storm";
    break;
  case "hello":
  default:
    path = "/";
    break;
}

const latencies = [];
let errors = 0;
let wrong = 0;

function validResponse(status, body) {
  if (scenario === "hello") return status === 200 && body === "hello from kiln";
  if (scenario === "static" || scenario === "static-file") return status === 200 && body.length > 0;
  if (scenario === "stream" || scenario === "chunked-stream") {
    return status === 200 && body === "kiln-stream-chunk-1\nkiln-stream-chunk-2\nkiln-stream-chunk-3\nkiln-stream-chunk-4\n";
  }
  if (scenario === "churn" || scenario === "connection-churn") return status === 200 && body === "churn ok";
  if (scenario === "upload" || scenario === "large-upload") {
    try { return status === 200 && JSON.parse(body).ok === true && JSON.parse(body).received === 262144; } catch { return false; }
  }
  if (scenario === "json-1k" || scenario === "json-echo-1k" || scenario === "json-64k" || scenario === "json-echo-64k") {
    try {
      const value = JSON.parse(body);
      const expectedKind = scenario.includes("64k") ? "64k" : "1k";
      return status === 200 && value.received?.kind === expectedKind && typeof value.received?.data === "string" &&
        value.received.data.length === (expectedKind === "64k" ? 65000 : 950) && value.bytes === Buffer.byteLength(bodyData) &&
        value.keys === 3 && value.sum === null;
    } catch { return false; }
  }
  if (scenario === "fs-small-reads") {
    try { const value = JSON.parse(body); return status === 200 && value.ok === true && value.files === 20 && value.totalBytes > 0; } catch { return false; }
  }
  if (scenario === "fs-large-read") {
    try { const value = JSON.parse(body); return status === 200 && value.ok === true && value.bytes > 0; } catch { return false; }
  }
  if (scenario === "fs-stat-storm") {
    try { const value = JSON.parse(body); return status === 200 && value.ok === true && value.count === 50 && value.totalSize > 0; } catch { return false; }
  }
  return status === 200;
}

function once() {
  return new Promise((resolve) => {
    const started = process.hrtime.bigint();
    let settled = false;
    const req = http.request(
      { host: "127.0.0.1", port, method, path, agent, headers },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => {
          if (settled) return;
          settled = true;
          const body = Buffer.concat(chunks).toString("utf8");
          if (!validResponse(response.statusCode, body)) wrong += 1;
          latencies.push(Number(process.hrtime.bigint() - started) / 1e6);
          resolve(true);
        });
      },
    );
    req.setTimeout(Math.max(1000, Math.min(10000, durationMs + 1000)), () => req.destroy(new Error("request timeout")));
    req.on("error", () => { if (!settled) { settled = true; resolve(false); } });
    if (bodyData) {
      req.write(bodyData);
    }
    req.end();
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
  scenario,
  requests: latencies.length,
  errors,
  wrong,
  seconds: Math.round(elapsed * 1000) / 1000,
  rps: Math.round(latencies.length / elapsed),
  p50_ms: percentile(latencies, 0.5),
  p95_ms: percentile(latencies, 0.95),
  p99_ms: percentile(latencies, 0.99),
}));
if (errors !== 0 || wrong !== 0 || latencies.length === 0) process.exitCode = 1;
