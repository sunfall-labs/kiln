// Saturate a single route, so throughput reports that route's service time rather than the queue.
import http from "node:http";
const [durationMs, conns, port, path, method] = [
  Number(process.argv[2] || 3000), Number(process.argv[3] || 16),
  Number(process.argv[4] || 18088), process.argv[5] || "/", process.argv[6] || "GET",
];
const agent = new http.Agent({ keepAlive: true, maxSockets: conns });
const body = method === "POST" ? JSON.stringify({ hello: "world" }) : null;
let done = 0, stop = false;
const one = () => new Promise((resolve) => {
  const req = http.request({ host: "127.0.0.1", port, path, method, agent,
    headers: body ? { "content-type": "application/json", "content-length": Buffer.byteLength(body) } : {} },
    (res) => { res.resume(); res.on("end", () => { done++; resolve(); }); });
  req.on("error", () => resolve());
  if (body) req.write(body);
  req.end();
});
const worker = async () => { while (!stop) await one(); };
const started = Date.now();
setTimeout(() => { stop = true; }, durationMs);
await Promise.all(Array.from({ length: conns }, worker));
console.log(JSON.stringify({ path, rps: Math.round(done / ((Date.now() - started) / 1000)) }));
