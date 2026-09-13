// What `require("node:stream")` publishes, and what classes/functions answer.
const stream = require("node:stream");

console.log("typeof stream:", typeof stream);
console.log("has Readable:", typeof stream.Readable === "function");
console.log("has Writable:", typeof stream.Writable === "function");
console.log("has finished:", typeof stream.finished === "function");
console.log("has addAbortSignal:", typeof stream.addAbortSignal === "function");

const names = Object.getOwnPropertyNames(stream).sort();
for (const name of names) {
  if (name !== "prototype" && name !== "length" && name !== "name") {
    console.log(name, typeof stream[name]);
  }
}

const r = new stream.Readable();
r.push("readable probe chunk");
console.log("read chunk:", r.read().toString() === "readable probe chunk");

let received = "";
const w = new stream.Writable({
  write(chunk, enc, cb) {
    received += chunk.toString();
    cb();
  }
});
w.write("writable probe chunk");
w.end();
console.log("wrote chunk:", received === "writable probe chunk");
console.log("writableEnded:", w.writableEnded);

let finishedCalled = false;
stream.finished(w, () => {
  finishedCalled = true;
});
console.log("finished called:", finishedCalled);
