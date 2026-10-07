// A `SharedArrayBuffer` sent to workers is the same memory in each realm (realms-and-concurrency
// issue 06): `Atomics.add` from three workers leaves exactly their total, a worker's
// `Atomics.wait` sleeps until the parent's `Atomics.notify` on the same cell, and a plain write in
// a worker is seen by the parent once the worker has said so. The native backend's
// `SharedArrayBuffer` and `Atomics` arrived with this; both engines print the same.
const { Worker, isMainThread, parentPort, workerData } = require("node:worker_threads");

const ADDS = 500;

if (isMainThread) {
  (async () => {
    const shared = new SharedArrayBuffer(16, { maxByteLength: 32 });
    const cells = new Int32Array(shared);
    console.log("shared", Object.prototype.toString.call(shared), shared instanceof SharedArrayBuffer,
      shared instanceof ArrayBuffer, shared.byteLength, shared.growable, shared.maxByteLength);
    const adders = [0, 1, 2].map(() => new Worker(__filename, { workerData: { shared, role: "add" } }));
    await Promise.all(adders.map((worker) => new Promise((resolve) => worker.once("exit", resolve))));
    console.log("counter", Atomics.load(cells, 0));

    const waiter = new Worker(__filename, { workerData: { shared, role: "wait" } });
    const answers = [];
    waiter.on("message", (message) => {
      if (message !== "waiting") {
        answers.push(message);
        return;
      }
      const poll = () => {
        if (Atomics.load(cells, 2) !== 1) return setTimeout(poll, 1);
        // The ready flag precedes Atomics.wait, so it cannot prove the waiter
        // has registered. Notify until the real wait queue reports a wakeup.
        const woken = Atomics.notify(cells, 1);
        if (woken === 0) return setTimeout(poll, 1);
        Atomics.store(cells, 1, 1);
        Atomics.store(cells, 2, 2);
        Atomics.notify(cells, 2);
        answers.push(`woke ${woken}`);
      };
      poll();
    });
    await new Promise((resolve) => waiter.once("exit", resolve));
    console.log(answers.join("\n"));
    console.log("written by the worker", cells[3]);
    console.log("not shared", Atomics.notify(new Int32Array(4), 0), Atomics.wait(cells, 0, 1, 0));
  })();
} else {
  const cells = new Int32Array(workerData.shared);
  if (workerData.role === "add") {
    for (let i = 0; i < ADDS; i++) Atomics.add(cells, 0, 1);
  } else {
    parentPort.postMessage("waiting");
    Atomics.store(cells, 2, 1);
    const first = Atomics.wait(cells, 1, 0);
    // Wait for the parent to publish the changed value before the second wait.
    while (Atomics.load(cells, 2) !== 2) Atomics.wait(cells, 2, 1);
    const second = Atomics.wait(cells, 1, 0);
    cells[3] = 42;
    parentPort.postMessage(`first ${first}, second ${second}`);
  }
}
