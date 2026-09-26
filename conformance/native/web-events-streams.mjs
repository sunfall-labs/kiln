// The WHATWG event and stream classes node puts on every global object: real in both engines
// since 2026-09-19 (the interpreter published hollow stubs, the native backend nothing). Every
// line here is node-identical in both engines; see crates/semantics/kiln-builtins/src/web_events_streams.ts.
const e = new Event("ping", { cancelable: true }); console.log(e.type, e.cancelable, e.defaultPrevented, e.eventPhase, Event.AT_TARGET, Object.keys(e).length, String(e), typeof e.timeStamp);
const t = new EventTarget(); const seen = [];
t.addEventListener("ping", (ev) => { seen.push("a:" + (ev.target === t) + ":" + (ev.currentTarget === t)); ev.preventDefault(); });
t.addEventListener("ping", { handleEvent(ev) { seen.push("obj"); } }, { once: true });
console.log(t.dispatchEvent(e), t.dispatchEvent(new Event("ping")), seen.join(","), e.defaultPrevented);
const c = new CustomEvent("x", { detail: { n: 1 } }); console.log(c.detail.n, c instanceof Event, c.type);
const { port1, port2 } = new MessageChannel(); port2.onmessage = (m) => { console.log("port2 got", m.data.a, m instanceof MessageEvent); port1.close(); }; port1.postMessage({ a: 1 });
const bc1 = new BroadcastChannel("k"), bc2 = new BroadcastChannel("k"); bc2.onmessage = (m) => { console.log("bc", m.data); bc1.close(); bc2.close(); }; bc1.postMessage("hello");
console.log(new CountQueuingStrategy({ highWaterMark: 2 }).size(), new ByteLengthQueuingStrategy({ highWaterMark: 16 }).size(new Uint8Array(3)));
const rs = new ReadableStream({ start(ctl) { ctl.enqueue("a"); ctl.enqueue("b"); ctl.close(); } });
const chunks = []; for await (const ch of rs) chunks.push(ch); console.log(chunks.join("+"), rs.locked);
const ts = new TransformStream({ transform(chunk, ctl) { ctl.enqueue(chunk.toUpperCase()); }, flush(ctl) { ctl.enqueue("!"); } });
const out = []; const ws = new WritableStream({ write(chunk) { out.push(chunk); } });
await ReadableStream.from(["x", "y"]).pipeThrough(ts).pipeTo(ws); console.log(out.join(""));
const r2 = new ReadableStream({ pull(ctl) { ctl.enqueue(1); ctl.close(); } }); const reader = r2.getReader(); console.log(JSON.stringify(await reader.read()), JSON.stringify(await reader.read()), r2.locked); reader.releaseLock(); console.log(r2.locked);
const [t1, t2] = ReadableStream.from([1, 2, 3]).tee(); const a1 = []; for await (const v of t1) a1.push(v); const a2 = []; for await (const v of t2) a2.push(v); console.log(a1.join(), a2.join());
const w = new WritableStream({ write(chunk) { if (chunk === "bad") throw new Error("nope"); } }, { highWaterMark: 1 }); const wr = w.getWriter(); console.log(wr.desiredSize); await wr.write("ok"); try { await wr.write("bad"); } catch (err) { console.log("write failed:", err.message); } console.log(await wr.closed.catch((e) => "closed rejected: " + e.message));
console.log(new Response("hi").body instanceof ReadableStream, Object.prototype.toString.call(new ReadableStream()), typeof ReadableStreamDefaultReader, typeof WritableStreamDefaultWriter);
