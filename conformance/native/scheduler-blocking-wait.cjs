// A scheduled top-level realm blocks its lane in Atomics.wait while a worker notifies it.
// The native-realms harness runs several copies together to exercise replacement lanes.
const { Worker, isMainThread, workerData } = require("node:worker_threads");

if (isMainThread) {
  const shared = new SharedArrayBuffer(4);
  const cell = new Int32Array(shared);
  new Worker(__filename, { workerData: shared });
  console.log("wait", Atomics.wait(cell, 0, 0, 5000));
} else {
  const cell = new Int32Array(workerData);
  Atomics.store(cell, 0, 1);
  Atomics.notify(cell, 0);
}
