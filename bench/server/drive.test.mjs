import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";
import test from "node:test";

const drive = join(import.meta.dirname, "drive.mjs");
const pipelineDrive = join(import.meta.dirname, "drive-pipeline.mjs");

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

function runPipeline(port, durationMs = 100) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [pipelineDrive, String(durationMs), "1", String(port)], {
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

test("drive rejects same-length JSON echo corruption", async () => {
  const fixture = await serverScript(`
    import http from "node:http";
    const server = http.createServer((req, res) => {
      req.resume(); req.on("end", () => res.end(JSON.stringify({received:{kind:"1k",data:"y".repeat(950),nums:[1,2,3,4,5]},bytes:1009,keys:3,sum:null})));
    });
    server.listen(0, "127.0.0.1", () => console.log(server.address().port));
  `);
  const { child, port } = await new Promise((resolve) => {
    const child = spawn(process.execPath, [fixture.path], { stdio: ["ignore", "pipe", "inherit"] });
    child.stdout.once("data", (data) => resolve({ child, port: Number(data) }));
  });
  try {
    const result = await runDrive(port, "json-echo-1k");
    const report = JSON.parse(result.stdout);
    assert.notEqual(result.code, 0);
    assert.ok(report.wrong > 0);
    assert.equal(report.errors, 0);
  } finally {
    await stopServer(child);
    await rm(fixture.dir, { recursive: true, force: true });
  }
});

test("drive settles a truncated response as an error", async () => {
  const fixture = await serverScript(`
    import http from "node:http";
    const server = http.createServer((req, res) => {
      res.writeHead(200, {"content-length":"32"}); res.write("partial"); res.socket.destroy();
    });
    server.listen(0, "127.0.0.1", () => console.log(server.address().port));
  `);
  const { child, port } = await new Promise((resolve) => {
    const child = spawn(process.execPath, [fixture.path], { stdio: ["ignore", "pipe", "inherit"] });
    child.stdout.once("data", (data) => resolve({ child, port: Number(data) }));
  });
  try {
    const result = await runDrive(port);
    const report = JSON.parse(result.stdout);
    assert.notEqual(result.code, 0);
    assert.ok(report.errors > 0);
  } finally {
    await stopServer(child);
    await rm(fixture.dir, { recursive: true, force: true });
  }
});

test("pipeline driver validates fixed-length and chunked responses", async () => {
  for (const framing of ["length", "chunked"]) {
    const fixture = await serverScript(`
      import http from "node:http";
      const server = http.createServer((_req, res) => {
        if (${JSON.stringify(framing)} === "chunked") {
          res.writeHead(200, {"transfer-encoding":"chunked"});
          res.write("hello "); res.end("from kiln");
        } else {
          res.writeHead(200, {"content-length":"15"}); res.end("hello from kiln");
        }
      });
      server.listen(0, "127.0.0.1", () => console.log(server.address().port));
    `);
    const { child, port } = await new Promise((resolve) => {
      const child = spawn(process.execPath, [fixture.path], { stdio: ["ignore", "pipe", "inherit"] });
      child.stdout.once("data", (data) => resolve({ child, port: Number(data) }));
    });
    try {
      const result = await runPipeline(port);
      const report = JSON.parse(result.stdout);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(report.pipeline_depth, 8);
      assert.ok(report.requests >= 8);
      assert.equal(report.errors, 0);
      assert.equal(report.wrong, 0);
    } finally {
      await stopServer(child);
      await rm(fixture.dir, { recursive: true, force: true });
    }
  }
});

test("pipeline driver rejects same-status wrong bodies", async () => {
  const fixture = await serverScript(`
    import http from "node:http";
    const server = http.createServer((_req, res) => {
      res.writeHead(200, {"content-length":"5"}); res.end("wrong");
    });
    server.listen(0, "127.0.0.1", () => console.log(server.address().port));
  `);
  const { child, port } = await new Promise((resolve) => {
    const child = spawn(process.execPath, [fixture.path], { stdio: ["ignore", "pipe", "inherit"] });
    child.stdout.once("data", (data) => resolve({ child, port: Number(data) }));
  });
  try {
    const result = await runPipeline(port);
    const report = JSON.parse(result.stdout);
    assert.notEqual(result.code, 0);
    assert.equal(report.errors, 0);
    assert.ok(report.wrong > 0);
  } finally {
    await stopServer(child);
    await rm(fixture.dir, { recursive: true, force: true });
  }
});
