import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";
import test from "node:test";

const drive = join(import.meta.dirname, "drive.mjs");

function runDrive(port, scenario = "hello") {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [drive, "100", "1", String(port), scenario], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.on("close", (code, signal) => resolve({ code, signal, stdout, stderr }));
  });
}

async function serverScript(body) {
  const dir = await mkdtemp(join(tmpdir(), "kiln-drive-test-"));
  const path = join(dir, "server.mjs");
  await writeFile(path, body);
  return { dir, path };
}

async function stopServer(child) {
  child.kill("SIGTERM");
  await new Promise((resolve) => child.once("close", resolve));
}

test("drive marks a wrong response and exits nonzero", async () => {
  const fixture = await serverScript(`
    import http from "node:http";
    const server = http.createServer((req, res) => { res.end("wrong"); });
    server.listen(0, "127.0.0.1", () => console.log(server.address().port));
  `);
  const portPromise = new Promise((resolve) => {
    const child = spawn(process.execPath, [fixture.path], { stdio: ["ignore", "pipe", "inherit"] });
    child.stdout.once("data", (data) => resolve({ child, port: Number(data) }));
  });
  const { child, port } = await portPromise;
  try {
    const result = await runDrive(port);
    const report = JSON.parse(result.stdout);
    assert.notEqual(result.code, 0);
    assert.equal(report.errors, 0);
    assert.ok(report.wrong > 0);
  } finally {
    await stopServer(child);
    await rm(fixture.dir, { recursive: true, force: true });
  }
});

test("drive fails cleanly when the server is unavailable", async () => {
  const result = await runDrive(1);
  const report = JSON.parse(result.stdout);
  assert.notEqual(result.code, 0);
  assert.ok(report.errors > 0);
  assert.equal(report.requests, 0);
});
