// A sole Node-style listener receives message data directly; replacing it or switching to an
// EventTarget listener must still preserve this binding, listener order and microtask order.
const { Worker, isMainThread, parentPort } = require("node:worker_threads");

if (isMainThread) {
  const worker = new Worker(__filename);
  const seen = [];
  worker.on("message", (message) => {
    seen.push(message);
    if (seen.length === 3) worker.postMessage(2);
    else if (seen.length === 4) worker.postMessage(3);
    else if (seen.length === 5) {
      console.log(JSON.stringify(seen));
      worker.terminate();
    }
  });
  worker.postMessage(1);
} else {
  function first(value) {
    parentPort.postMessage(["first", value, this === parentPort]);
    process.nextTick(() => parentPort.postMessage(["tick", value]));
    Promise.resolve().then(() => parentPort.postMessage(["job", value]));
    parentPort.off("message", first);
    parentPort.on("message", second);
  }
  function second(value) {
    parentPort.postMessage(["second", value, this === parentPort]);
    parentPort.removeAllListeners("message");
    parentPort.addEventListener("message", (event) => {
      parentPort.postMessage(["event", event.data, event.target === parentPort]);
    });
  }
  parentPort.on("message", first);
}
