import { Readable } from "node:stream";

// A deferred read keeps the same state captured before user listeners run. Narrowing the
// capture's scope must not replace it with a later lookup of the receiver's current state.
const flow = new Readable({ read() {} });
flow.on("data", () => {});
await new Promise((resolve) => process.nextTick(resolve));
const original = flow._readableState;
original.reading = false;
original.highWaterMark = 23;
const replacement = new Readable({ highWaterMark: 99 })._readableState;
flow._read = (size) => console.log("captured read", size);
flow.on("data", () => { flow._readableState = replacement; });
flow.push(Buffer.from("chunk"));
await new Promise((resolve) => process.nextTick(resolve));
console.log("state retained", flow._readableState === replacement, original.reading);

const paused = new Readable({ read() {} });
const saved = paused._readableState;
let reads = 0;
Object.defineProperty(paused, "_readableState", { get() { reads++; return saved; } });
paused.push(Buffer.from("buffered"));
console.log("data state lookups", reads);
reads = 0;
paused.push(null);
console.log("EOF state lookups", reads, saved.ended, saved.endEmitted);
const output = paused.read();
console.log("buffer before end", output.toString(), paused.readableEnded);
