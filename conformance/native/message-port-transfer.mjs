// Message ports, transfer lists and the structured clone: interpreter and native must agree
// line for line (issue 14 of .scratch/realms-and-concurrency). The native backend copies plain
// data in one call (`__kilnCloneData`) and falls back to the shared TypeScript clone for
// anything else; the interpreter always takes the TypeScript clone, so every line here that
// clones plain data is also a check that the two paths agree.
const log = [];
const show = (label, fn) => {
  try { log.push(label + " " + fn()); } catch (e) { log.push(label + " throws " + e.name + " " + e.message); }
};

// Plain data, large enough to spill the fast path's table off the C stack, with sharing.
const cats = [];
for (let i = 0; i < 13; i += 1) cats.push({ code: (i * 3) | 0, label: "category-" + i });
const items = [];
for (let i = 0; i < 3000; i += 1) items.push({ id: i, name: "item-" + i, tags: ["t" + (i % 13)], price: (i % 100) / 4, cat: cats[i % 13] });
const copy = structuredClone(items);
let sum = 0;
const distinct = new Set();
for (const it of copy) { sum += it.id + it.tags[0].length + it.price + it.cat.code; distinct.add(it.cat); }
log.push("graph " + copy.length + " " + sum + " " + distinct.size + " " + (copy[5].cat !== cats[5]));

// Mixed values: the fast path must hand over to the general clone, not half-copy.
const mixed = { when: new Date(7), re: /x+/g, set: new Set([1, 2]), nested: { deep: [1, { d: new Map([["k", 1]]) }] } };
const mc = structuredClone(mixed);
log.push("mixed " + mc.when.getTime() + " " + mc.re.flags + " " + mc.set.size + " " + mc.nested.deep[1].d.get("k"));
const getter = { get g() { return { made: 1 }; }, v: [1, 2] };
log.push("getter " + JSON.stringify(structuredClone(getter)));
const frozen = Object.freeze({ a: 1, b: [2] });
const fc = structuredClone(frozen);
fc.a = 3;
log.push("frozen " + Object.isFrozen(fc) + " " + fc.a);
const cyclic = { list: [] };
cyclic.list.push(cyclic, cyclic.list);
const cc = structuredClone(cyclic);
log.push("cycle " + (cc.list[0] === cc) + " " + (cc.list[1] === cc.list));
const holes = [1, , 3];
log.push("holes " + (1 in structuredClone(holes)) + " " + structuredClone(holes).length);
const hidden = { v: 1 };
Object.defineProperty(hidden, "h", { value: 2, enumerable: false });
log.push("non-enumerable " + JSON.stringify(Object.getOwnPropertyNames(structuredClone(hidden))));
const sym = Symbol("s");
log.push("symbol key " + Object.getOwnPropertySymbols(structuredClone({ a: 1, [sym]: 2 })).length);
show("function inside", () => structuredClone({ ok: 1, list: [1, { f() {} }] }));

// Detach: every view over a transferred buffer is empty, and the bytes moved.
const buf = new ArrayBuffer(8);
const view = new Uint8Array(buf, 2, 4);
view[0] = 9;
const dv = new DataView(buf);
const moved = structuredClone({ view, buf }, { transfer: [buf] });
log.push("sender " + buf.byteLength + " " + buf.detached + " " + view.length + " " + view.byteOffset + " " + view[0]);
log.push("receiver " + moved.buf.byteLength + " " + moved.view.length + " " + moved.view[0] + " " + (moved.view.buffer === moved.buf));
// Error kinds only: the two engines word these differently, and so does node.
try { dv.getUint8(0); } catch (e) { log.push("dataview over detached " + e.name); }
try { buf.slice(0); } catch (e) { log.push("slice detached " + e.name); }
const grown = new ArrayBuffer(4);
new Uint8Array(grown)[3] = 5;
const g2 = grown.transfer(8);
log.push("transfer(8) " + g2.byteLength + " " + new Uint8Array(g2)[3] + " " + new Uint8Array(g2)[7] + " " + grown.detached);
const shrunk = g2.transferToFixedLength(2);
log.push("transferToFixedLength(2) " + shrunk.byteLength + " " + g2.byteLength);

// Delivery: batches, checkpoints, a transferred port, and close.
const a = new MessageChannel(), b = new MessageChannel();
a.port2.onmessage = async (e) => {
  log.push("A" + e.data);
  process.nextTick(() => log.push("At" + e.data));
  await null;
  log.push("Aw" + e.data);
};
b.port2.addEventListener("message", (e) => log.push("B" + e.data));
a.port1.postMessage(1); b.port1.postMessage(1); a.port1.postMessage(2); b.port1.postMessage(2);
setImmediate(() => log.push("immediate"));
const inner = new MessageChannel();
inner.port2.onmessage = (e) => log.push("inner " + e.data);
const carrier = new MessageChannel();
carrier.port2.onmessage = (e) => {
  const port = e.ports[0];
  log.push("carried " + (e.data.port === port) + " " + e.ports.length);
  port.postMessage("via moved port");
  carrier.port1.close();
};
carrier.port1.postMessage({ port: inner.port1 }, [inner.port1]);
log.push("moved port closed " + (inner.port1.postMessage("dropped") === undefined));
const bc1 = new BroadcastChannel("n"), bc2 = new BroadcastChannel("n"), bc3 = new BroadcastChannel("n");
const payload = { n: 1 };
let seen2 = null;
bc2.onmessage = (e) => { seen2 = e.data; };
bc3.onmessage = (e) => { log.push("broadcast " + e.data.n + " " + (e.data !== seen2)); bc1.close(); bc2.close(); bc3.close(); };
bc1.postMessage(payload);

setTimeout(() => {
  for (const ch of [a, b, inner]) ch.port1.close();
  inner.port2.close();
  console.log(log.join("\n"));
}, 30);
