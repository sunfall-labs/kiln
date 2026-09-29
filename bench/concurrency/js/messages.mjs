// 2. Messages between two threads (node, bun): a worker_threads Worker that echoes or
// acknowledges, driven from the main thread.
//
//   small-pingpong      -- a short object, sent and echoed back; time per round trip.
//   small-oneway        -- 100,000 short objects sent without waiting; one reply at the end.
//   graph-1mb           -- a ~1 MB object graph (structured clone), acknowledged with its sum.
//   ab16-copy           -- a 16 MB ArrayBuffer, copied there and copied back.
//   ab16-transfer       -- the same buffer, transferred there and transferred back.
//   ab1k-transfer       -- a 1 KB ArrayBuffer, transferred out and back.
//   ab100-transfer      -- a 100 MB ArrayBuffer, transferred out and back.
import { Worker } from "node:worker_threads";
import { arg, buildGraph, emit, fillBuffer, perOp, smallMessage, touchBuffer, walkGraph } from "./common.mjs";

const budget = Number(arg(0, 1000));
const onewayCount = Number(arg(1, 100000));
const only = arg(2, "all");
const want = (name) => only === "all" || only.split(",").includes(name);
const worker = new Worker(new URL("./messages-worker.mjs", import.meta.url));
let pending = null;
worker.on("message", (value) => {
  const resolve = pending;
  pending = null;
  resolve(value);
});
function request(message, transfer) {
  return new Promise((resolve) => {
    pending = resolve;
    worker.postMessage(message, transfer);
  });
}

// small ping-pong
if (want("small-pingpong")) {
  let i = 0;
  let ok = true;
  const per = await perOp(budget, 2000, async () => {
    const echo = await request({ kind: "echo", body: smallMessage(i) });
    if (echo.body.id !== i || echo.body.tags[1] !== "b") ok = false;
    i += 1;
  });
  emit({ case: "small-pingpong", us_per_op: per * 1000, ops_per_s: 1000 / per, checksum: ok ? "ok" : "mismatch" });
}

// small one-way
if (want("small-oneway")) {
  const count = onewayCount;
  let total = 0;
  const per = await perOp(budget, 1, async () => {
    const done = new Promise((resolve) => { pending = resolve; });
    worker.postMessage({ kind: "count-start", count });
    for (let i = 0; i < count; i += 1) worker.postMessage(smallMessage(i));
    total = await done;
  });
  emit({ case: "small-oneway", msgs_per_s: count / (per / 1000), checksum: String(total) });
}

// 1 MB object graph
if (want("graph-1mb")) {
  const graph = buildGraph();
  let sum = 0;
  const per = await perOp(budget, 4, async () => {
    sum = await request({ kind: "walk", graph });
  });
  emit({ case: "graph-1mb", ms_per_op: per, checksum: String(sum) });
}

// 16 MB ArrayBuffer, copied vs transferred
for (const variant of ["ab16-copy", "ab16-transfer"].filter(want)) {
  let buffer = new ArrayBuffer(16 << 20);
  fillBuffer(new Uint8Array(buffer));
  let check = 0;
  const transfer = variant === "ab16-transfer";
  const per = await perOp(budget, 2, async () => {
    const reply = await request({ kind: "buffer", buffer, transfer }, transfer ? [buffer] : undefined);
    buffer = reply.buffer;
    check = reply.check;
  });
  emit({ case: variant, ms_per_op: per, checksum: String(check) });
}

// The ownership handoff should not depend on payload size. Touch only boundary bytes here and
// in the worker, outside the transfer mechanism being measured.
for (const [variant, size] of [["ab1k-transfer", 1 << 10], ["ab100-transfer", 100 << 20]].filter(([name]) => want(name))) {
  let buffer = new ArrayBuffer(size);
  let bytes = new Uint8Array(buffer);
  bytes[0] = 17;
  bytes[size - 1] = 29;
  let ok = true;
  const per = await perOp(budget, 2, async () => {
    const outgoing = buffer;
    const reply = await request({ kind: "buffer-edge", buffer }, [buffer]);
    buffer = reply.buffer;
    bytes = new Uint8Array(buffer);
    if (outgoing.byteLength !== 0 || !reply.check || buffer.byteLength !== size ||
        bytes[0] !== 17 || bytes[size - 1] !== 29) ok = false;
  });
  emit({ case: variant, ms_per_op: per, checksum: ok ? "ok" : "mismatch" });
}

await worker.terminate();
