// `node:worker_threads` on realms (realms-and-concurrency issue 06): a worker is a realm of its own
// on a thread of the runtime's realm host, and every value between the two is a structured clone
// carried by the runtime's ports. Both engines run this, so what the two print is one answer:
// messages in order, `online` before the first and `exit` after the last, `workerData` and `argv`
// as given, a transferred buffer detached in the sender with its bytes intact in the worker, an
// exception in the worker reported to the parent's `error` event, `process.exit` ending only the
// worker, and `terminate()` stopping an idle worker with code 1.
const { Worker, isMainThread, parentPort, workerData, threadId, MessageChannel } = require("node:worker_threads");

if (isMainThread) {
  (async () => {
    const worker = new Worker(__filename, { workerData: { role: "echo", tags: new Set(["a", "b"]) }, argv: ["x", 1] });
    const events = [];
    worker.on("online", () => events.push("online"));
    const first = new Promise((resolve) => worker.once("message", resolve));
    worker.on("message", (message) => events.push(`message ${JSON.stringify(message)}`));
    const buffer = new ArrayBuffer(4);
    new Uint8Array(buffer).set([1, 2, 3, 4]);
    worker.postMessage({ buffer, when: new Date(86400000) }, [buffer]);
    events.push(`sender after transfer ${buffer.byteLength} ${buffer.detached}`);
    // The port goes once the first reply is in: the two channels are ordered only by that.
    await first;
    const { port1, port2 } = new MessageChannel();
    // The worker may exit before its reply on this separate port is delivered.
    const exited = new Promise((resolve) => worker.once("exit", resolve));
    worker.postMessage({ port: port2 }, [port2]);
    const overPort = await new Promise((resolve) => port1.once("message", resolve));
    events.push(`over a transferred port ${JSON.stringify(overPort)}`);
    port1.close();
    events.push(`exit ${await exited}`);
    console.log(events.join("\n"));

    for (const role of ["throw", "exit", "idle"]) {
      const other = new Worker(__filename, { workerData: { role } });
      other.on("error", (error) => console.log(role, "error", error.name, error.message, error instanceof RangeError));
      other.on("message", (message) => console.log(role, "message", message));
      if (role === "idle") {
        await new Promise((resolve) => other.once("message", resolve));
        console.log(role, "terminate answered", await other.terminate());
      } else {
        console.log(role, "exit", await new Promise((resolve) => other.once("exit", resolve)));
      }
    }
    console.log("parent done", threadId);
  })();
} else if (workerData.role === "echo") {
  let received = 0;
  parentPort.on("message", (message) => {
    if (message.buffer) {
      parentPort.postMessage({
        thread: threadId,
        argv: process.argv.slice(2),
        tags: Array.from(workerData.tags),
        bytes: Array.from(new Uint8Array(message.buffer)),
        day: message.when.getTime(),
      });
    } else {
      message.port.postMessage({ from: "worker" });
      message.port.close();
    }
    if (++received === 2) parentPort.close();
  });
} else if (workerData.role === "throw") {
  throw new RangeError("out of range in the worker");
} else if (workerData.role === "exit") {
  parentPort.postMessage("before exit");
  process.exit(5);
} else {
  parentPort.on("message", () => {});
  parentPort.postMessage("waiting");
}
