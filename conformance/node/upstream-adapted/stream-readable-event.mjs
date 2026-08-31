// Adapted from Node v24.7.0 parallel/test-stream-readable-event.js.
// The private `common.mustCall` harness is replaced by explicit counters; API behavior is intact.
import { Readable } from "node:stream";

let lateNotReading = 0;
{
  const r = new Readable({ highWaterMark: 3 });
  r._read = () => { throw new Error("unexpected _read"); };
  r.push(Buffer.from("blerg"));
  setTimeout(() => {
    if (r._readableState.reading) throw new Error("first stream is reading");
    r.on("readable", () => { lateNotReading += 1; });
  }, 1);
}

let readCalls = 0;
let lateReading = 0;
{
  const r = new Readable({ highWaterMark: 3 });
  r._read = () => { readCalls += 1; };
  r.push(Buffer.from("bl"));
  setTimeout(() => {
    if (!r._readableState.reading) throw new Error("second stream is not reading");
    r.on("readable", () => { lateReading += 1; });
  }, 1);
}

let lateEof = 0;
{
  const r = new Readable({ highWaterMark: 30 });
  r._read = () => { throw new Error("unexpected EOF _read"); };
  r.push(Buffer.from("blerg"));
  r.push(null);
  setTimeout(() => {
    if (r._readableState.reading) throw new Error("EOF stream is reading");
    r.on("readable", () => { lateEof += 1; });
  }, 1);
}

const underlyingData = ["", "x", "y", "", "z"];
const result = [];
let ended = 0;
let pullCalls = 0;
{
  const r = new Readable({ encoding: "utf8" });
  r._read = function() {
    pullCalls += 1;
    process.nextTick(() => {
      this.push(underlyingData.length ? underlyingData.shift() : null);
    });
  };
  r.on("readable", () => {
    const data = r.read();
    if (data !== null) result.push(data);
  });
  r.on("end", () => { ended += 1; });
}

{
  const r = new Readable();
  r._read = () => {};
  r.on("data", () => {});
  r.removeAllListeners();
  if (r.eventNames().length !== 0) throw new Error("event names remain");
}

setTimeout(() => {
  if (lateNotReading !== 1) throw new Error(`late not-reading count ${lateNotReading}`);
  if (lateReading !== 1) throw new Error(`late reading count ${lateReading}`);
  if (lateEof !== 1) throw new Error(`late EOF count ${lateEof}`);
  if (readCalls !== 0) throw new Error(`second read calls ${readCalls}`);
  if (pullCalls === 0) throw new Error("producer was not pulled");
  if (result.join(",") !== "x,y,z") throw new Error(`result ${result.join(",")}`);
  if (ended !== 1) throw new Error(`end count ${ended}`);
  console.log("pass");
}, 2);
