// The mixed-workload client for app.mjs. Always node, whatever is serving. Each worker walks a
// weighted rotation of the app's routes over one keep-alive connection, checks every status and
// body shape (a runtime that answers fast and wrong would otherwise post the best number), and
// reports rps and latency overall and per route.
//
// usage: node drive-mix.mjs [durationMs=5000] [concurrency=16] [port=18088]
import http from "node:http";

const durationMs = Number(process.argv[2] ?? 5000);
const concurrency = Number(process.argv[3] ?? 16);
const port = Number(process.argv[4] ?? 18088);
const agent = new http.Agent({ keepAlive: true, maxSockets: concurrency });

const echoBody = JSON.stringify({ name: "kiln", values: [1, 2, 3, 4, 5, 6, 7, 8], nested: { a: 1, b: "two" } });

// Weighted: the cheap routes dominate, as they do in a real service.
const mix = [
  { name: "root", method: "GET", path: "/", weight: 3, check: (s, b) => s === 200 && b === "hello from the app" },
  { name: "json", method: "GET", path: "/json", weight: 3, check: (s, b) => s === 200 && JSON.parse(b).ok === true },
  { name: "user", method: "GET", path: "/users/42", weight: 2, check: (s, b) => s === 200 && JSON.parse(b).id === 42 },
  { name: "user-fields", method: "GET", path: "/users/7?fields=id,name", weight: 1, check: (s, b) => s === 200 && JSON.parse(b).name === "user-7" && JSON.parse(b).tags === undefined },
  { name: "user-404", method: "GET", path: "/users/999", weight: 1, check: (s, b) => s === 404 },
  { name: "echo", method: "POST", path: "/echo", body: echoBody, weight: 2, check: (s, b) => s === 200 && JSON.parse(b).sum === 36 },
  { name: "big", method: "GET", path: "/big", weight: 1, check: (s, b) => s === 200 && b.length > 8000 && b.endsWith("end") },
  { name: "headers", method: "GET", path: "/headers", weight: 1, check: (s, b) => s === 200 && JSON.parse(b).custom === "abc-123" },
  { name: "async", method: "GET", path: "/async", weight: 1, check: (s, b) => s === 200 && JSON.parse(b).waited === true },
  { name: "404", method: "GET", path: "/nope", weight: 1, check: (s) => s === 404 },
];
const rotation = mix.flatMap((route) => Array.from({ length: route.weight }, () => route));

const perRoute = new Map(mix.map((route) => [route.name, { count: 0, latencies: [] }]));
let errors = 0;
let wrong = 0;
let total = 0;

function once(route) {
  return new Promise((resolve) => {
    const started = process.hrtime.bigint();
    const request = http.request(
      {
        host: "127.0.0.1",
        port,
        method: route.method,
        path: route.path,
        agent,
        headers: {
          accept: "application/json",
          "x-request-id": "abc-123",
          ...(route.body ? { "content-type": "application/json", "content-length": String(Buffer.byteLength(route.body)) } : {}),
        },
      },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => {
          const body = Buffer.concat(chunks).toString("utf8");
          const elapsed = Number(process.hrtime.bigint() - started) / 1e6;
          let ok = false;
          try { ok = route.check(response.statusCode, body); } catch { ok = false; }
          if (!ok) wrong += 1;
          const stats = perRoute.get(route.name);
          stats.count += 1;
          stats.latencies.push(elapsed);
          total += 1;
          resolve(true);
        });
      },
    );
    request.on("error", () => resolve(false));
    if (route.body) request.write(route.body);
    request.end();
  });
}

async function worker(offset, deadline) {
  let index = offset;
  while (Date.now() < deadline) {
    const route = rotation[index % rotation.length];
    index += 1;
    if (!(await once(route))) errors += 1;
  }
}

function percentile(sorted, fraction) {
  if (sorted.length === 0) return null;
  const index = Math.min(sorted.length - 1, Math.floor(sorted.length * fraction));
  return Math.round(sorted[index] * 1000) / 1000;
}

const startedAt = Date.now();
const deadline = startedAt + durationMs;
await Promise.all(Array.from({ length: concurrency }, (_, offset) => worker(offset, deadline)));
const elapsed = (Date.now() - startedAt) / 1000;

const all = [...perRoute.values()].flatMap((stats) => stats.latencies).sort((a, b) => a - b);
const routes = {};
for (const [name, stats] of perRoute) {
  const sorted = stats.latencies.sort((a, b) => a - b);
  routes[name] = { rps: Math.round(stats.count / elapsed), p50_ms: percentile(sorted, 0.5), p99_ms: percentile(sorted, 0.99) };
}
console.log(JSON.stringify({
  requests: total,
  errors,
  wrong,
  seconds: Math.round(elapsed * 1000) / 1000,
  rps: Math.round(total / elapsed),
  p50_ms: percentile(all, 0.5),
  p99_ms: percentile(all, 0.99),
  routes,
}));
