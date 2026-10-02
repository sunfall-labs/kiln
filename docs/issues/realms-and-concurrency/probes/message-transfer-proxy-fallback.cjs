// A Proxy with a transferred buffer stays on the TypeScript clone path. Node throws
// DataCloneError; kiln's engines currently disagree there (see issue 21).
const { Worker, isMainThread, parentPort } = require("node:worker_threads");
if (isMainThread) {
  const worker = new Worker(__filename);
  const buffer = new ArrayBuffer(8);
  try {
    worker.postMessage({ value: new Proxy({ buffer }, {}) }, [buffer]);
    console.log("accepted", buffer.byteLength);
  } catch (error) {
    console.log(error.name, buffer.byteLength);
  }
  worker.terminate();
} else {
  parentPort.on("message", () => parentPort.close());
}
