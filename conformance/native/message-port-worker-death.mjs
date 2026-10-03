// kiln-native: clock=wall
// A port transferred into a worker closes when that worker ends -- by `process.exit`, an
// uncaught error or `terminate()` -- and its partner here sees `close`, as in node. The runtime
// closes every port a worker holds when its realm ends; a port in transit is held by no realm.
import { Worker, MessageChannel, isMainThread, workerData } from "node:worker_threads";

// The parent's loop runs in an async function, not top-level `await`, so this probe asks only
// about realm death (top-level await has its own probes).
async function main() {
  for (const how of ["exit", "throw", "terminate"]) {
    for (const listenFirst of [true, false]) {
      const { port1, port2 } = new MessageChannel();
      if (listenFirst) port1.on("message", () => {});
      const worker = new Worker(new URL(import.meta.url), { workerData: { port: port2, how }, transferList: [port2] });
      worker.on("error", () => {});
      const closed = await new Promise((resolve) => {
        const timer = setTimeout(() => { port1.close(); resolve(false); }, 5000);
        if (!listenFirst) port1.on("message", () => {});
        port1.on("close", () => { clearTimeout(timer); resolve(true); });
        if (how === "terminate") setTimeout(() => worker.terminate(), 50);
      });
      console.log(how, listenFirst ? "listening before" : "listening after", "partner closed:", closed);
    }
  }
}

if (isMainThread) {
  main();
} else {
  // Hold the port, then end the way the parent asked.
  workerData.port.on("message", () => {});
  if (workerData.how === "exit") process.exit(3);
  else if (workerData.how === "throw") setTimeout(() => { throw new Error("boom"); }, 10);
  else setInterval(() => {}, 1000);
}
