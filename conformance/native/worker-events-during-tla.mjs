// A worker's message, a transferred port's `close` and the worker's `exit` reach the entry
// module while its top-level await is still pending, ahead of a guard timer that would
// otherwise win. The worker runs this same module (no `eval: true`: a natively built program
// has no compiler at run time). A virtual-clock realm once jumped its paused clock to the
// guard as soon as it parked, so a worker on another thread never had the chance to post
// first and every line below read "timeout".
import { Worker, MessageChannel, isMainThread, parentPort, workerData } from "node:worker_threads";

// Resolve with `label` when `event` fires, or with "timeout" when the guard timer does.
const first = (target, event, label) =>
  new Promise((resolve) => {
    const guard = setTimeout(() => resolve(event + " timeout"), 3000);
    target.once(event, (value) => {
      clearTimeout(guard);
      resolve(label(value));
    });
  });

if (isMainThread) {
  const { port1, port2 } = new MessageChannel();
  const received = [];
  port1.on("message", (m) => received.push(m));
  const worker = new Worker(new URL(import.meta.url), { workerData: { port: port2 }, transferList: [port2] });
  const exit = first(worker, "exit", (code) => "exit " + code);
  const closed = first(port1, "close", () => "port close");
  console.log(await first(worker, "message", (m) => "worker message " + m));
  console.log(await closed);
  console.log(await exit);
  console.log("port messages", received.join(","));
} else {
  const { port } = workerData;
  port.postMessage("from worker");
  parentPort.postMessage("hello");
  port.close();
  process.exit(3);
}
