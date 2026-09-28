// `BroadcastChannel` across realms (realms-and-concurrency issue 06): a post reaches every other
// channel of the name, in this realm and in each worker, each with its own copy, and a closed
// channel hears nothing. Both engines print the same.
const { Worker, isMainThread, parentPort, workerData, BroadcastChannel } = require("node:worker_threads");

if (isMainThread) {
  const channel = new BroadcastChannel("probe");
  const listener = new BroadcastChannel("probe");
  const heard = [];
  const workers = [1, 2].map((n) => new Worker(__filename, { workerData: n }));
  let ready = 0;
  let replies = 0;
  let mainReceived = false;
  // The listener and the two workers run on different realms. Their deliveries have no
  // relative order, so wait for all three before comparing the printed transcript.
  const finishIfDone = () => {
    if (!mainReceived || replies !== workers.length) return;
    console.log(heard.sort().join("\n"));
    channel.close();
    listener.close();
  };
  listener.onmessage = (event) => {
    heard.push(`main heard ${JSON.stringify(event.data)}`);
    mainReceived = true;
    finishIfDone();
  };
  for (const worker of workers) {
    worker.on("message", (message) => {
      if (message === "ready") {
        if (++ready === workers.length) channel.postMessage({ n: 7, list: [1] });
        return;
      }
      heard.push(message);
      replies++;
      finishIfDone();
    });
  }
} else {
  const channel = new BroadcastChannel("probe");
  channel.onmessage = (event) => {
    parentPort.postMessage(`worker ${workerData} heard ${JSON.stringify(event.data)}`);
    channel.close();
  };
  parentPort.postMessage("ready");
}
