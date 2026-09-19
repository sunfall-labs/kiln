// An application-shaped probe: an HTTP JSON service and a client for it in one process, with a
// typed error, a URL route match, `timers/promises`, an AbortController, and a top-level await
// on a live server. (A rejected top-level await beside a live handle hung the native process on
// 2026-09-19; that shape exits non-zero by design, so it lives in kiln-native's tests instead.)
import http from "node:http";
import { setTimeout as sleep } from "node:timers/promises";

class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.name = "HttpError";
    this.status = status;
  }
}

const users = new Map([[1, { id: 1, name: "Ada" }], [2, { id: 2, name: "Linus" }]]);
const server = http.createServer(async (req, res) => {
  try {
    if (req.method !== "GET") throw new HttpError(405, "method not allowed");
    const m = /^\/users\/(\d+)$/.exec(req.url.split("?")[0]);
    if (!m) throw new HttpError(404, "not found");
    await sleep(1);
    const u = users.get(Number(m[1]));
    if (!u) throw new HttpError(404, "no such user");
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify(u));
  } catch (e) {
    const status = e instanceof HttpError ? e.status : 500;
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: e.name, message: e.message }));
  }
});
await new Promise((resolve) => server.listen(0, resolve));
const port = server.address().port;
const get = (p) =>
  new Promise((resolve, reject) => {
    const req = http.get({ host: "127.0.0.1", port, path: p }, (r) => {
      let body = "";
      r.setEncoding("utf8");
      r.on("data", (c) => (body += c));
      r.on("end", () => resolve([r.statusCode, body]));
    });
    req.on("error", reject);
  });
console.log(await get("/users/1"));
console.log(await get("/users/9"));
console.log(await get("/nope?x=1"));
const ac = new AbortController();
setTimeout(() => ac.abort(), 5);
try {
  await sleep(1000, null, { signal: ac.signal });
} catch (e) {
  console.log("aborted:", e.name);
}
server.close();
console.log("done");
