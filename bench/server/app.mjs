// A small service rather than a hello world: routing on the URL, query parsing, JSON in and
// out, a POST body, a response large enough to need more than one packet, header echo, and an
// async handler. Run under node, deno, bun, or kiln; `drive-mix.mjs` is the client.
//
// The port is a constant so the program stays inside the surface every runtime shares; pass a
// different one by editing the copy you run.
import http from "node:http";
import { readFile, stat } from "node:fs/promises";

const port = 18088;
const big = "kiln ".repeat(1638) + "end"; // ~8 KiB
const users = new Map();
for (let id = 1; id <= 100; id += 1) {
  users.set(String(id), { id, name: `user-${id}`, tags: ["a", "b", "c"], score: id * 7 % 101 });
}
let served = 0;

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

function json(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": String(Buffer.byteLength(body)),
  });
  res.end(body);
}

function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    req.on("data", (chunk) => { chunks.push(chunk); });
    req.on("end", () => { resolve(Buffer.concat(chunks).toString("utf8")); });
  });
}

const server = http.createServer(async (req, res) => {
  served += 1;
  const url = new URL(req.url, "http://localhost");
  const path = url.pathname;
  if (req.method === "GET" && path === "/") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("hello from the app");
    return;
  }
  if (req.method === "GET" && path === "/json") {
    json(res, 200, { ok: true, served, now: Date.now(), items: [1, 2, 3, 4, 5] });
    return;
  }
  if (req.method === "GET" && path.startsWith("/users/")) {
    const id = path.slice("/users/".length);
    const user = users.get(id);
    if (user === undefined) {
      json(res, 404, { error: "no such user", id });
      return;
    }
    const fields = url.searchParams.get("fields");
    if (fields === null) {
      json(res, 200, user);
    } else {
      const picked = {};
      for (const field of fields.split(",")) {
        if (field in user) picked[field] = user[field];
      }
      json(res, 200, picked);
    }
    return;
  }
  if (req.method === "POST" && path === "/echo") {
    const text = await readBody(req);
    let parsed;
    try {
      parsed = JSON.parse(text);
    } catch {
      json(res, 400, { error: "bad json" });
      return;
    }
    json(res, 200, {
      received: parsed,
      bytes: Buffer.byteLength(text),
      keys: Object.keys(parsed).length,
      sum: Array.isArray(parsed.values) ? parsed.values.reduce((a, b) => a + b, 0) : null,
    });
    return;
  }
  if (req.method === "GET" && path === "/big") {
    res.writeHead(200, { "content-type": "text/plain", "content-length": String(big.length) });
    res.end(big);
    return;
  }
  if (req.method === "GET" && path === "/headers") {
    res.setHeader("x-served", String(served));
    res.setHeader("cache-control", "no-store");
    json(res, 200, {
      agent: req.headers["user-agent"] ?? null,
      accept: req.headers["accept"] ?? null,
      custom: req.headers["x-request-id"] ?? null,
      host: req.headers["host"] ?? null,
    });
    return;
  }
  if (req.method === "GET" && path === "/async") {
    await new Promise((resolve) => setTimeout(resolve, 0));
    json(res, 200, { waited: true, served });
    return;
  }
  if (req.method === "GET" && path === "/static") {
    try {
      const data = await readFile(staticPath, "utf8");
      res.writeHead(200, {
        "content-type": "text/plain",
        "content-length": String(Buffer.byteLength(data)),
      });
      res.end(data);
    } catch (err) {
      json(res, 500, { error: String(err) });
    }
    return;
  }
  if (req.method === "GET" && path === "/stream") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.write("kiln-stream-chunk-1\n");
    res.write("kiln-stream-chunk-2\n");
    res.write("kiln-stream-chunk-3\n");
    res.end("kiln-stream-chunk-4\n");
    return;
  }
  if (req.method === "POST" && path === "/upload") {
    let received = 0;
    req.on("data", (chunk) => { received += chunk.length; });
    req.on("end", () => {
      json(res, 200, { ok: true, received });
    });
    return;
  }
  if (req.method === "POST" && path === "/multicore") {
    const text = await readBody(req);
    const chunks = [];
    for (let i = 0; i < 16; i++) {
      chunks.push((i * 31 + text.length) % 256);
    }
    const hashes = await (chunks.parallelMap ? chunks.parallelMap(x => {
      let h = x;
      for (let j = 0; j < 1000; j++) {
        h = ((h << 5) - h + j) & 0xffffffff;
      }
      return h;
    }) : chunks.map(x => {
      let h = x;
      for (let j = 0; j < 1000; j++) {
        h = ((h << 5) - h + j) & 0xffffffff;
      }
      return h;
    }));
    json(res, 200, { ok: true, hashes });
    return;
  }
  if (req.method === "GET" && path === "/churn") {
    res.writeHead(200, { "content-type": "text/plain", "connection": "close" });
    res.end("churn ok");
    return;
  }
  if (req.method === "GET" && path === "/fs/small-reads") {
    try {
      let totalBytes = 0;
      for (let i = 0; i < smallFilePaths.length; i++) {
        const c = await readFile(smallFilePaths[i], "utf8");
        totalBytes += c.length;
      }
      json(res, 200, { ok: true, files: smallFilePaths.length, totalBytes });
    } catch (err) {
      json(res, 500, { error: String(err) });
    }
    return;
  }
  if (req.method === "GET" && path === "/fs/large-read") {
    try {
      const data = await readFile(largeFilePath);
      json(res, 200, { ok: true, bytes: data.length });
    } catch (err) {
      json(res, 500, { error: String(err) });
    }
    return;
  }
  if (req.method === "GET" && path === "/fs/stat-storm") {
    try {
      let totalSize = 0;
      for (let i = 0; i < statPaths.length; i++) {
        const info = await stat(statPaths[i]);
        totalSize += info.size;
      }
      json(res, 200, { ok: true, count: statPaths.length, totalSize });
    } catch (err) {
      json(res, 500, { error: String(err) });
    }
    return;
  }
  json(res, 404, { error: "not found", path });
});

server.listen(port, "127.0.0.1", () => {
  console.log("listening");
});
