import { parentPort } from "node:worker_threads";
import { lookups } from "./common.mjs";

let table = null;
parentPort.on("message", (message) => {
  if (message.kind === "ping") parentPort.postMessage(0);
  else if (message.kind === "table") {
    table = new Float64Array(message.buffer);
    parentPort.postMessage(table.length);
  } else if (message.kind === "query") {
    parentPort.postMessage(lookups(table, message.seed, message.queries));
  }
});
