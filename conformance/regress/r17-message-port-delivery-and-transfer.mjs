// MessagePort delivery order, transfer lists and the structured clone, as node has them.
// Every line is node's output; see the manifest entry for what each part catches.
import { markAsUntransferable, isMarkedAsUntransferable } from "node:worker_threads";

const log = [];
const show = (label, fn) => {
  try { log.push(label + " " + fn()); } catch (e) { log.push(label + " throws " + e.name + " " + e.message + " " + e.code); }
};

// 1. Delivery: posting order per port, a whole checkpoint (ticks, then promise jobs, then an
//    async handler's later awaits) between two messages, and each port's queue as one batch.
const a = new MessageChannel(), b = new MessageChannel();
a.port2.onmessage = async (e) => {
  log.push("A" + e.data);
  process.nextTick(() => log.push("At" + e.data));
  Promise.resolve().then(() => log.push("Ap" + e.data));
  await null; log.push("Aw1-" + e.data);
  await null; log.push("Aw2-" + e.data);
};
b.port2.onmessage = (e) => {
  log.push("B" + e.data);
  queueMicrotask(() => { log.push("Bq" + e.data); process.nextTick(() => log.push("Bqt" + e.data)); });
};
a.port1.postMessage(1); b.port1.postMessage(1); a.port1.postMessage(2); b.port1.postMessage(2); a.port1.postMessage(3);

// 2. An `onmessage` handler is a listener in the position it was first set in.
const e = new MessageChannel();
e.port2.addEventListener("message", () => log.push("E1"));
e.port2.onmessage = () => log.push("E-on");
e.port2.addEventListener("message", () => log.push("E2"));
e.port2.onmessage = (ev) => log.push("E-on2 " + (ev.target === e.port2) + " " + ev.ports.length);
e.port1.postMessage(0);

// 3. Transfer: the sender's buffer is detached, the receiver gets the bytes, a view in the
//    message is rebuilt over the moved buffer, and a failed clone detaches nothing.
const u8 = new Uint8Array([1, 2, 3, 4]);
const t = new MessageChannel();
t.port2.onmessage = (ev) => {
  const d = ev.data;
  log.push("T " + d.view.length + " " + d.view[2] + " " + (d.view.buffer === d.buf) + " " + d.buf.byteLength);
  t.port1.close();
};
log.push("returns " + t.port1.postMessage({ view: u8, buf: u8.buffer }, [u8.buffer]));
log.push("sender " + u8.length + " " + u8.byteLength + " " + u8.buffer.byteLength + " " + u8.buffer.detached + " " + u8[0]);
const kept = new ArrayBuffer(8);
show("failed clone", () => t.port1.postMessage({ kept, f() {} }, [kept]));
log.push("kept " + kept.byteLength + " " + kept.detached);
const opt = new ArrayBuffer(3);
structuredClone(opt, { transfer: [opt] });
log.push("options form " + opt.byteLength);
show("dup", () => { const x = new ArrayBuffer(1); return structuredClone(x, { transfer: [x, x] }); });
show("detached in list", () => structuredClone(1, { transfer: [opt] }));
show("detached value", () => structuredClone(opt));
show("not transferable", () => structuredClone(1, { transfer: [{}] }));
show("bad list", () => t.port1.postMessage(1, 5));
show("source port", () => t.port1.postMessage(1, [t.port1]));
show("port not listed", () => structuredClone(t.port1));
const moved = new ArrayBuffer(16).transfer(4);
log.push("transfer(4) " + moved.byteLength);
show("transfer detached", () => opt.transfer());
try { new Uint8Array(opt); } catch (err) { log.push("view over detached " + err.name); }

// 4. markAsUntransferable: refused in a transfer list, still cloneable.
const pinned = new ArrayBuffer(8);
markAsUntransferable(pinned);
show("untransferable", () => structuredClone(pinned, { transfer: [pinned] }));
log.push("pinned " + isMarkedAsUntransferable(pinned) + " " + pinned.byteLength + " " + structuredClone(pinned).byteLength);

// 5. The clone: shared references, cycles, holes, keys, and what it refuses.
const shared = { s: 1 };
const g = { a: shared, b: shared, list: [shared, 2, , "x", null, undefined, 10n], m: new Map([[shared, [shared]]]) };
g.self = g;
const c = structuredClone(g);
log.push("graph " + (c.a === c.b) + " " + (c.list[0] === c.a) + " " + (c.self === c) + " " + (1 in c.list) + " " + (c.m.get(c.a)[0] === c.a) + " " + typeof c.list[6]);
const proto = structuredClone(JSON.parse('{"__proto__": {"x": 1}, "y": 2}'));
log.push("own __proto__ " + (Object.getPrototypeOf(proto) === Object.prototype) + " " + Object.keys(proto).join());
class K { constructor() { this.v = [1]; } }
log.push("instance " + (Object.getPrototypeOf(structuredClone(new K())) === Object.prototype));
show("function", () => structuredClone({ f() {} }));
show("symbol", () => structuredClone([Symbol("q")]));
show("promise", () => structuredClone(Promise.resolve()));

setTimeout(() => {
  for (const ch of [a, b, e]) ch.port1.close();
  console.log(log.join("\n"));
}, 20);
