// `BroadcastChannel` across realms (realms-and-concurrency issue 06): a post reaches every other
// channel of the name, in this realm and in each worker, each with its own copy, and a closed
// channel hears nothing. Both engines print the same.
const { Worker, isMainThread, parentPort, workerData, BroadcastChannel } = require("node:worker_threads");

if (isMainThread) {
  const channel = new BroadcastChannel("probe");
  const listener = new BroadcastChannel("probe");
  const heard = [];
  listener.onmessage = (event) => heard.push(`main heard ${JSON.stringify(event.data)}`);
  const workers = [1, 2].map((n) => new Worker(__filename, { workerData: n }));
  let ready = 0;
  let replies = 0;
  for (const worker of workers) {
    worker.on("message", (message) => {
      if (message === "ready") {
        if (++ready === workers.length) channel.postMessage({ n: 7, list: [1] });
        return;
      }
      heard.push(message);
      if (++replies === workers.length) {
        console.log(heard.sort().join("\n"));
        channel.close();
        listener.close();
      }
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
