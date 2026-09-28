// Closing a port in its first callback discards the rest of a queued messages.
const { Worker, isMainThread, parentPort, MessageChannel } = require("node:worker_threads");

if (isMainThread) {
  const worker = new Worker(__filename);
  const channel = new MessageChannel();
  const seen = [];
  worker.on("message", (value) => seen.push(value));
  worker.on("exit", () => {
    channel.port2.close();
    console.log(seen.join("\n"));
  });
  worker.postMessage(channel.port1, [channel.port1]);
  channel.port2.postMessage(0);
  channel.port2.postMessage(1);
  channel.port2.postMessage(2);
} else {
  parentPort.once("message", (port) => {
    port.on("message", (value) => {
      parentPort.postMessage("closed " + value);
      port.close();
      parentPort.close();
    });
  });
}
