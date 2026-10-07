// A sparse array's inherited numeric getter is not an own element of the clone. Exercise
// both the guarded plain path and the transfer-list fallback.
const { Worker, isMainThread, parentPort } = require("node:worker_threads");

if (isMainThread) {
  const worker = new Worker(__filename);
  const answers = [];
  let inheritedReads = 0;
  Object.defineProperty(Array.prototype, "0", {
    configurable: true,
    get() { inheritedReads++; return 7; },
  });
  worker.postMessage([,]);
  worker.postMessage([,], [new ArrayBuffer(0)]);
  delete Array.prototype[0];

  worker.on("message", (answer) => answers.push(answer));
  worker.on("exit", () => {
    console.log(answers.join("\n"));
    console.log("reads", inheritedReads);
  });
} else {
  let received = 0;
  parentPort.on("message", (array) => {
    parentPort.postMessage(Object.hasOwn(array, 0) + " " + String(array[0]));
    if (++received === 2) parentPort.close();
  });
}
