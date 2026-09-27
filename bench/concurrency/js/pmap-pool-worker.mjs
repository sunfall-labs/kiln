import { parentPort } from "node:worker_threads";

parentPort.on("message", (part) => {
  if (part === null) return parentPort.postMessage(0);
  for (let i = 0; i < part.length; i += 1) {
    let y = part[i];
    for (let k = 0; k < 16; k += 1) y = (y * 31 + k) % 1000003;
    part[i] = y;
  }
  parentPort.postMessage(part, [part.buffer]);
});
