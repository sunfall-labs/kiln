// Raw HTTP/1.1 client that sends batches of pipelined GET requests on persistent sockets.
// The server is expected to answer each request with status 200 and the fixed hello body.
//
// usage: node drive-pipeline.mjs [durationMs=5000] [connections=16] [port=18088]
import net from "node:net";

const durationMs = Number(process.argv[2] ?? 5000);
const concurrency = Number(process.argv[3] ?? 16);
const port = Number(process.argv[4] ?? 18088);
const pipelineDepth = 8;
const bodyExpected = "hello from kiln";
const maxResponseBodyBytes = 64 * 1024;
const maxBufferedBytes = 1024 * 1024;
if (![durationMs, concurrency, port].every(Number.isSafeInteger) || durationMs <= 0 || concurrency <= 0 || port <= 0 || port > 65535) {
  throw new Error("duration, connections, and port must be positive integers");
}

const request = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: */*\r\nConnection: keep-alive\r\n\r\n";
const requestBatch = request.repeat(pipelineDepth);
const latencies = [];
let errors = 0;
let wrong = 0;
let total = 0;
let stopped = false;
const states = [];

function findCrlf(buffer, start = 0) {
  return buffer.indexOf("\r\n", start, "latin1");
}

// Returns one complete response, null while more bytes are needed. Interim responses are
// consumed without advancing the request queue; final responses carry a body for validation.
function takeResponse(buffer) {
  const headerEnd = buffer.indexOf("\r\n\r\n", 0, "latin1");
  if (headerEnd < 0) return null;
  if (headerEnd > 16 * 1024) throw new Error("HTTP response headers are too large");
  const headerText = buffer.subarray(0, headerEnd).toString("latin1");
  const lines = headerText.split("\r\n");
  const statusMatch = /^HTTP\/1\.[01] ([0-9]{3})(?: |$)/.exec(lines[0] ?? "");
  if (!statusMatch) throw new Error("invalid HTTP status line");
  const status = Number(statusMatch[1]);
  const headers = new Map();
  for (const line of lines.slice(1)) {
    const colon = line.indexOf(":");
    if (colon <= 0) throw new Error("invalid HTTP response header");
    const name = line.slice(0, colon).trim().toLowerCase();
    const value = line.slice(colon + 1).trim();
    headers.set(name, headers.has(name) ? `${headers.get(name)}, ${value}` : value);
  }

  const bodyStart = headerEnd + 4;
  if (status >= 100 && status < 200) return { consumed: bodyStart, interim: true };
  if (status === 204 || status === 304) {
    return { consumed: bodyStart, interim: false, status, body: Buffer.alloc(0) };
  }

  const transferEncoding = headers.get("transfer-encoding") ?? "";
  if (transferEncoding.toLowerCase().split(/,\s*/).includes("chunked")) {
    let cursor = bodyStart;
    const chunks = [];
    let bodyBytes = 0;
    while (true) {
      const lineEnd = findCrlf(buffer, cursor);
      if (lineEnd < 0) return null;
      const sizeText = buffer.subarray(cursor, lineEnd).toString("latin1").split(";", 1)[0].trim();
      if (!/^[0-9a-f]+$/i.test(sizeText)) throw new Error("invalid HTTP chunk size");
      const size = Number.parseInt(sizeText, 16);
      if (!Number.isSafeInteger(size)) throw new Error("HTTP chunk size is too large");
      if (bodyBytes + size > maxResponseBodyBytes) {
        throw new Error("HTTP response body is too large");
      }
      cursor = lineEnd + 2;
      if (size === 0) {
        if (buffer.length < cursor + 2) return null;
        if (buffer[cursor] === 13 && buffer[cursor + 1] === 10) {
          return { consumed: cursor + 2, interim: false, status, body: Buffer.concat(chunks) };
        }
        const trailersEnd = buffer.indexOf("\r\n\r\n", cursor, "latin1");
        if (trailersEnd < 0) return null;
        return { consumed: trailersEnd + 4, interim: false, status, body: Buffer.concat(chunks) };
      }
      if (buffer.length < cursor + size + 2) return null;
      if (buffer[cursor + size] !== 13 || buffer[cursor + size + 1] !== 10) {
        throw new Error("invalid HTTP chunk terminator");
      }
      chunks.push(buffer.subarray(cursor, cursor + size));
      bodyBytes += size;
      cursor += size + 2;
    }
  }

  const contentLengthText = headers.get("content-length");
  if (contentLengthText === undefined || !/^\d+$/.test(contentLengthText)) {
    throw new Error("persistent response has no valid Content-Length or chunked encoding");
  }
  const contentLength = Number(contentLengthText);
  if (!Number.isSafeInteger(contentLength) || contentLength > maxResponseBodyBytes) {
    throw new Error("HTTP Content-Length is too large");
  }
  if (buffer.length < bodyStart + contentLength) return null;
  return {
    consumed: bodyStart + contentLength,
    interim: false,
    status,
    body: buffer.subarray(bodyStart, bodyStart + contentLength),
  };
}

function openConnection() {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: "127.0.0.1", port });
    const state = { socket, buffer: Buffer.alloc(0), pending: [], failure: null, connected: false };
    states.push(state);
    const fail = (error) => {
      if (state.failure) return;
      state.failure = error;
      if (!state.connected) reject(error);
      for (const item of state.pending.splice(0)) item.reject(error);
    };
    state.fail = fail;
    socket.setNoDelay(true);
    socket.setTimeout(Math.max(1000, Math.min(10000, durationMs + 1000)), () => {
      socket.destroy(new Error("pipeline socket timed out"));
    });
    socket.once("connect", () => {
      state.connected = true;
      resolve(state);
    });
    socket.on("error", fail);
    socket.on("close", () => {
      if (state.pending.length) fail(new Error("pipeline socket closed before all responses arrived"));
      else state.failure ??= new Error("pipeline socket closed");
    });
    socket.on("data", (chunk) => {
      state.buffer = Buffer.concat([state.buffer, chunk]);
      try {
        if (state.buffer.length > maxBufferedBytes) throw new Error("HTTP response buffer exceeded its limit");
        while (true) {
          const response = takeResponse(state.buffer);
          if (!response) break;
          state.buffer = state.buffer.subarray(response.consumed);
          if (response.interim) continue;
          const item = state.pending.shift();
          if (!item) throw new Error("received an unsolicited HTTP response");
          item.resolve({
            status: response.status,
            body: response.body,
            elapsed_ms: Number(process.hrtime.bigint() - item.sent) / 1e6,
          });
        }
      } catch (error) {
        fail(error instanceof Error ? error : new Error(String(error)));
        socket.destroy();
      }
    });
  });
}

function sendBatch(state) {
  if (state.failure || state.socket.destroyed) return Promise.reject(state.failure ?? new Error("pipeline socket is closed"));
  const sent = process.hrtime.bigint();
  const batch = Array.from({ length: pipelineDepth }, () => new Promise((resolve, reject) => {
    state.pending.push({ resolve, reject, sent });
  }));
  state.socket.write(requestBatch, (error) => {
    if (error) state.fail(error);
  });
  return Promise.all(batch);
}

function percentile(sorted, fraction) {
  if (sorted.length === 0) return null;
  const index = Math.min(sorted.length - 1, Math.floor(sorted.length * fraction));
  return Math.round(sorted[index] * 1000) / 1000;
}

async function worker(state, deadline) {
  try {
    while (!stopped && Date.now() < deadline) {
      const responses = await sendBatch(state);
      for (const response of responses) {
        total += 1;
        if (response.status !== 200 || response.body.toString("utf8") !== bodyExpected) wrong += 1;
        latencies.push(response.elapsed_ms);
      }
    }
  } catch (error) {
    stopped = true;
    errors += 1;
    for (const other of states) if (!other.socket.destroyed) other.socket.destroy(error);
  } finally {
    if (state && !state.socket.destroyed) state.socket.end();
  }
}

try {
  await Promise.all(Array.from({ length: concurrency }, () => openConnection()));
} catch (error) {
  stopped = true;
  errors += 1;
  for (const state of states) if (!state.socket.destroyed) state.socket.destroy(error);
}
const started = Date.now();
const deadline = started + durationMs;
if (!stopped) await Promise.all(states.map((state) => worker(state, deadline)));
for (const state of states) state.socket.destroy();
const elapsed = Math.max((Date.now() - started) / 1000, 0.001);
latencies.sort((a, b) => a - b);
console.log(JSON.stringify({
  scenario: "pipeline",
  pipeline_depth: pipelineDepth,
  requests: total,
  errors,
  wrong,
  seconds: Math.round(elapsed * 1000) / 1000,
  rps: Math.round(total / elapsed),
  p50_ms: percentile(latencies, 0.5),
  p95_ms: percentile(latencies, 0.95),
  p99_ms: percentile(latencies, 0.99),
}));
if (errors !== 0 || wrong !== 0 || total === 0) process.exitCode = 1;
