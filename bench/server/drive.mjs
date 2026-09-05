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

function once() {
  return new Promise((resolve) => {
    const started = process.hrtime.bigint();
    const req = http.request(
      { host: "127.0.0.1", port, method, path, agent, headers },
      (response) => {
        response.resume();
        response.on("end", () => {
          latencies.push(Number(process.hrtime.bigint() - started) / 1e6);
          resolve(response.statusCode === 200);
        });
      },
    );
    req.on("error", () => resolve(false));
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
  seconds: Math.round(elapsed * 1000) / 1000,
  rps: Math.round(latencies.length / elapsed),
  p50_ms: percentile(latencies, 0.5),
  p95_ms: percentile(latencies, 0.95),
  p99_ms: percentile(latencies, 0.99),
}));
