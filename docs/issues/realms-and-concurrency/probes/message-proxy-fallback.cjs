// The native plain-data encoder must leave Proxy values to the TypeScript clone.
// Node throws DataCloneError. Kiln's current interpreter accepts this Proxy;
// the native fallback refuses Array.prototype.push on receiver_tag=11.
const { Worker, isMainThread, parentPort } = require("node:worker_threads");
if (isMainThread) {
  const worker = new Worker(__filename);
  try {
    worker.postMessage({ value: new Proxy({ answer: 1 }, {}) });
    console.log("accepted");
  } catch (error) {
    console.log(error.name);
  }
  worker.terminate();
} else {
  parentPort.on("message", ({ value }) => parentPort.postMessage(value.answer));
}
