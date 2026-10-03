// kiln-native: clock=wall
// The same events as worker-events-during-tla.mjs, from an async function rather than the
// entry's top-level await: a pending timer must not keep a parked realm from hearing a worker
// on another thread. The guard is long, so a drive that slept until it would be slow, and a
// drive that jumped its clock to it would print "timeout".
import { Worker, MessageChannel, isMainThread, parentPort, workerData } from "node:worker_threads";

const first = (target, event, label) =>
  new Promise((resolve) => {
    const guard = setTimeout(() => resolve(event + " timeout"), 8000);
    target.once(event, (value) => {
      clearTimeout(guard);
      resolve(label(value));
    });
  });

async function main() {
  const { port1, port2 } = new MessageChannel();
  port1.on("message", () => {});
  const worker = new Worker(new URL(import.meta.url), { workerData: { port: port2 }, transferList: [port2] });
  const online = first(worker, "online", () => "online");
  const exit = first(worker, "exit", (code) => "exit " + code);
  const closed = first(port1, "close", () => "port close");
  const message = first(worker, "message", (m) => "worker message " + m);
  console.log(await online);
  console.log(await message);
  console.log(await closed);
  console.log(await exit);
}

if (isMainThread) {
  main();
} else {
  parentPort.postMessage("hello");
  setTimeout(() => {
    workerData.port.close();
    process.exit(3);
  }, 20);
}
