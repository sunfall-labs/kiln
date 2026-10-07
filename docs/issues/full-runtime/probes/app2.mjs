// An HTTP JSON service with a typed error, a timeout, and a client hitting it.
import http from "node:http"; import { setTimeout as sleep } from "node:timers/promises"; import { URL } from "node:url";
class HttpError extends Error { constructor(status, m) { super(m); this.name = "HttpError"; this.status = status; } }
const users = new Map([[1, { id: 1, name: "Ada" }], [2, { id: 2, name: "Linus" }]]);
const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, "http://localhost");
  try {
    if (req.method !== "GET") throw new HttpError(405, "method not allowed");
    const m = url.pathname.match(/^\/users\/(\d+)$/);
    if (!m) throw new HttpError(404, "not found");
    await sleep(1);
    const u = users.get(Number(m[1])); if (!u) throw new HttpError(404, "no such user");
    res.writeHead(200, { "content-type": "application/json" }); res.end(JSON.stringify(u));
  } catch (e) {
    const status = e instanceof HttpError ? e.status : 500;
    res.writeHead(status, { "content-type": "application/json" }); res.end(JSON.stringify({ error: e.name, message: e.message }));
  }
});
await new Promise((r) => server.listen(0, r));
const port = server.address().port;
const get = (p) => new Promise((resolve, reject) => http.get(`http://127.0.0.1:${port}${p}`, (r) => { let b = ""; r.setEncoding("utf8"); r.on("data", (c) => b += c); r.on("end", () => resolve([r.statusCode, b])); }).on("error", reject));
console.log(await get("/users/1")); console.log(await get("/users/9")); console.log(await get("/nope"));
const ac = new AbortController(); setTimeout(() => ac.abort(), 5);
try { await sleep(1000, null, { signal: ac.signal }); } catch (e) { console.log("aborted:", e.name); }
server.close(); console.log("done");
