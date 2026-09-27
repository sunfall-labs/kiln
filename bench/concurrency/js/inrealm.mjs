// 2b. Messages inside one realm: the global MessageChannel, both ports on the same thread.
//
// This is the only message passing kiln has today (no worker_threads, no cross-realm ports), so
// it is measured in every JavaScript runtime and labelled "in-realm": it measures the
// structured clone and the event-loop hop, not a thread crossing. Same cases as messages.mjs.
import { arg, buildGraph, emit, fillBuffer, perOp, smallMessage, touchBuffer, walkGraph } from "./common.mjs";

const budget = Number(arg(0, 1000));
const onewayCount = Number(arg(1, 100000));
const only = arg(2, "all");
const want = (name) => only === "all" || only.split(",").includes(name);
const { port1, port2 } = new MessageChannel();

// port2 plays the worker.
let counting = 0;
let counted = 0;
let total = 0;
port2.onmessage = (event) => {
  const message = event.data;
  if (counting > 0) {
    total += message.qty;
    counted += 1;
    if (counted === counting) {
      counting = 0;
      port2.postMessage(total);
    }
    return;
  }
  if (message.kind === "echo") port2.postMessage(message);
  else if (message.kind === "count-start") { counting = message.count; counted = 0; total = 0; }
  else if (message.kind === "walk") port2.postMessage(walkGraph(message.graph));
  else if (message.kind === "buffer") {
    const check = touchBuffer(new Uint8Array(message.buffer));
    if (message.transfer) port2.postMessage({ buffer: message.buffer, check }, [message.buffer]);
    else port2.postMessage({ buffer: message.buffer, check });
  }
};

let pending = null;
port1.onmessage = (event) => {
  const resolve = pending;
  pending = null;
  resolve(event.data);
};
function request(message, transfer) {
  return new Promise((resolve) => {
    pending = resolve;
    if (transfer) port1.postMessage(message, transfer);
    else port1.postMessage(message);
  });
}

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
if (want("small-oneway")) {
  const count = onewayCount;
  let result = 0;
  const per = await perOp(budget, 1, async () => {
    const done = new Promise((resolve) => { pending = resolve; });
    port1.postMessage({ kind: "count-start", count });
    for (let i = 0; i < count; i += 1) port1.postMessage(smallMessage(i));
    result = await done;
  });
  emit({ case: "small-oneway", msgs_per_s: count / (per / 1000), checksum: String(result) });
}
if (want("graph-1mb")) {
  const graph = buildGraph();
  let sum = 0;
  const per = await perOp(budget, 4, async () => {
    sum = await request({ kind: "walk", graph });
  });
  emit({ case: "graph-1mb", ms_per_op: per, checksum: String(sum) });
}
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
port1.close();
port2.close();
