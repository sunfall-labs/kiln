// A callback can synchronously receive the next message, then transfer the port while more
// messages wait. The unconsumed prefix must follow the port ahead of later posts.
const { Worker, isMainThread, parentPort, MessageChannel, receiveMessageOnPort } = require("node:worker_threads");

if (isMainThread) {
  const worker = new Worker(__filename);
  const channel = new MessageChannel();
  const seen = [];
  worker.on("message", (message) => {
    if (message.port) {
      const port = message.port;
      port.on("message", (value) => {
        seen.push("moved " + value);
        if (value === 3) {
          port.close();
          channel.port2.close();
          worker.terminate().then(() => console.log(seen.join("\n")));
        }
      });
      channel.port2.postMessage(3);
    } else {
      seen.push(message);
    }
  });
  worker.postMessage(channel.port1, [channel.port1]);
  channel.port2.postMessage(0);
  channel.port2.postMessage(1);
  channel.port2.postMessage(2);
} else {
  parentPort.once("message", (port) => {
    port.on("message", (value) => {
      const next = receiveMessageOnPort(port);
      parentPort.postMessage("first " + value + " sync " + (next && next.message));
      parentPort.postMessage({ port }, [port]);
      parentPort.close();
    });
  });
}
