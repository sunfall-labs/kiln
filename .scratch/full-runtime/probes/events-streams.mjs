// EventEmitter + streams + Buffer: a line splitter transform, a pipeline, once(), Buffer encodings.
import { EventEmitter, once } from "node:events";
import { Readable, Transform, Writable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { Buffer } from "node:buffer";
class Ticker extends EventEmitter { start(n) { for (let i = 1; i <= n; i++) this.emit("tick", i); this.emit("done"); return this; } }
const t = new Ticker(); const seen = []; t.on("tick", (i) => seen.push(i)); t.once("done", () => seen.push("done")); t.start(3);
console.log("ticks:", seen.join(","), "| listeners:", t.listenerCount("tick"), "| names:", t.eventNames().join(","));
const p = once(t, "late"); setTimeout(() => t.emit("late", 42), 1); const [late] = await p; console.log("once:", late);
const chunks = []; const lines = new Transform({ transform(chunk, enc, cb) { this._rest = (this._rest ?? "") + chunk.toString(); const parts = this._rest.split("\n"); this._rest = parts.pop(); for (const l of parts) this.push(l.toUpperCase() + "\n"); cb(); }, flush(cb) { if (this._rest) this.push(this._rest.toUpperCase() + "\n"); cb(); } });
await pipeline(Readable.from(["ab\ncd", "e\nf", "gh"]), lines, new Writable({ write(chunk, enc, cb) { chunks.push(chunk.toString()); cb(); } }));
console.log("pipeline:", JSON.stringify(chunks.join("")));
const b = Buffer.from("héllo wörld", "utf8"); console.log("buf:", b.length, b.toString("hex").slice(0, 8), b.toString("base64"), Buffer.concat([b.subarray(0, 3), Buffer.from("!")]).toString(), b.equals(Buffer.from(b)), Buffer.byteLength("é"), b.readUInt16BE(0), Buffer.alloc(3, 1).toJSON().data.join(""));
