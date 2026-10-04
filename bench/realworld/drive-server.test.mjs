import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import http from "node:http";
import { once } from "node:events";
import test from "node:test";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

test("the sequential load client reuses its connection and verifies every body", async () => {
  let connections = 0;
  let requests = 0;
  const server = http.createServer((req, res) => {
    requests++;
    const url = new URL(req.url, "http://localhost");
    res.end(url.pathname === "/hello" ? "hello from realworld" :
      JSON.stringify({ sum: Number(url.searchParams.get("a")) + Number(url.searchParams.get("b")) }));
  });
  server.on("connection", () => connections++);
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  try {
    const { stdout } = await promisify(execFile)(process.execPath,
      [fileURLToPath(new URL("./drive-server.mjs", import.meta.url)), "100"],
      { env: { ...process.env, REALWORLD_PORT: String(server.address().port) }, timeout: 10000 });
    const result = JSON.parse(stdout);
    assert.equal(result.requests, 100);
    assert.equal(result.wrong, 0);
    assert.equal(requests, 100);
    assert.equal(connections, 1);
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});
